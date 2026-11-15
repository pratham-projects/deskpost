mod common;
mod r21_world;

use axum::http::Method;
use dp_core::api_key::hash_api_key;
use r21_world::*;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

fn pid(w: &W) -> String {
    w.project.id.to_string()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn create_returns_the_api_key_once_and_stores_only_its_hash(pool: PgPool) {
    let w = world(pool).await;
    let res = w
        .post(
            "/projects",
            &w.agent.token,
            json!({ "name": "  New One ", "slug": "new-one", "domain": "New.Example.com", "description": "d" }),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let d = data(&res);
    let key = d["apiKey"].as_str().unwrap().to_string();
    assert!(key.starts_with("sdk_"));
    assert_eq!(d["apiKeyPrefix"], key[..12]);
    assert_eq!(
        (
            d["name"].as_str(),
            d["domain"].as_str(),
            d["ownerUserId"].as_str()
        ),
        (
            Some("New One"),
            Some("new.example.com"),
            Some(w.agent.id.to_string().as_str())
        )
    );
    assert_eq!(
        (d["smtpPort"].as_i64(), d["smtpHost"].as_str()),
        (Some(587), Some(""))
    );
    assert_eq!(
        (d["hasSmtpPassword"].clone(), d["imapFolder"].clone()),
        (json!(false), json!("INBOX"))
    );
    assert_no_secrets(&res, &[]);

    let id = d["id"].as_str().unwrap();
    let hash: String = sqlx::query_scalar("SELECT api_key_hash FROM projects WHERE id = $1::uuid")
        .bind(id)
        .fetch_one(&w.t.pool)
        .await
        .unwrap();
    assert_eq!(hash, hash_api_key(&key));

    // The creator is an admin member; the key never comes back.
    let got = w.get(&format!("/projects/{id}"), &w.agent.token).await;
    assert_eq!(got.status, 200);
    assert!(!got.text().contains(&key) && data(&got).get("apiKey").is_none());
    let members = w
        .get(&format!("/projects/{id}/members"), &w.agent.token)
        .await;
    assert_eq!(data(&members)[0]["role"], "admin");
    assert_eq!(data(&members)[0]["canViewAllTickets"], true);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn create_validation_reports_every_bad_field(pool: PgPool) {
    let w = world(pool).await;
    let res = w
        .post(
            "/projects",
            &w.owner.token,
            json!({
                "name": "", "slug": "AB", "domain": "nodot", "description": "x".repeat(5001),
                "smtpEmail": "nope", "replyToEmail": "nope", "smtpHost": "localhost", "smtpPort": 0,
                "imapHost": "10.0.0.1", "imapPort": 70000, "smtpPassword": "p".repeat(513),
            }),
        )
        .await;
    assert_eq!(res.status, 422);
    assert_eq!(res.error_code(), "VALIDATION_ERROR");
    let details = &res.json()["error"]["details"];
    for f in [
        "name",
        "slug",
        "domain",
        "description",
        "smtpEmail",
        "replyToEmail",
        "smtpHost",
        "smtpPort",
        "imapHost",
        "imapPort",
        "smtpPassword",
    ] {
        assert!(details.get(f).is_some(), "missing {f}: {details}");
    }
    // Missing body fields and wrong types are 422 too, and nothing was created.
    for body in [json!({}), json!({ "name": 5, "slug": "abc" })] {
        assert_eq!(w.post("/projects", &w.owner.token, body).await.status, 422);
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM projects")
        .fetch_one(&w.t.pool)
        .await
        .unwrap();
    assert_eq!(n, 2);
    let anon = w
        .call(Method::POST, "/projects", None, Some(json!({})))
        .await;
    assert_eq!(anon.error_code(), "TOKEN_MISSING");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn slug_domain_and_mailbox_must_be_unique_and_a_failed_create_leaves_nothing(pool: PgPool) {
    let w = world(pool).await;
    let ok = w
        .post(
            "/projects",
            &w.owner.token,
            json!({ "name": "A", "slug": "first", "domain": "a.example.com", "smtpEmail": "Help@a.example.com" }),
        )
        .await;
    assert_eq!(ok.status, 201, "{}", ok.text());
    for (body, what) in [
        (json!({ "name": "B", "slug": "first" }), "slug"),
        (
            json!({ "name": "B", "slug": "second", "domain": "A.EXAMPLE.com" }),
            "domain",
        ),
        (
            json!({ "name": "B", "slug": "third", "smtpEmail": "help@A.example.com" }),
            "mailbox",
        ),
    ] {
        let res = w.post("/projects", &w.other.token, body).await;
        assert_eq!(
            (res.status.as_u16(), res.error_code().as_str()),
            (409, "CONFLICT"),
            "{what}"
        );
    }
    let leftovers: i64 =
        sqlx::query_scalar("SELECT count(*) FROM projects WHERE slug IN ('second', 'third')")
            .fetch_one(&w.t.pool)
            .await
            .unwrap();
    assert_eq!(
        leftovers, 0,
        "a rejected mailbox must not leave the project behind"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn list_and_get_are_tenant_scoped(pool: PgPool) {
    let w = world(pool).await;
    let list = w.get("/projects", &w.agent.token).await;
    assert_eq!(data(&list).as_array().unwrap().len(), 1);
    assert_eq!(list.json()["pagination"]["total"], 1);
    assert_eq!(data(&w.get("/projects", &w.root.token).await), json!([]));
    assert_eq!(
        w.get("/projects?status=active&limit=1&offset=0", &w.owner.token)
            .await
            .status,
        200
    );
    for q in ["limit=0", "limit=201", "offset=-1", "status=bogus"] {
        assert_eq!(
            w.get(&format!("/projects?{q}"), &w.owner.token)
                .await
                .status,
            422,
            "{q}"
        );
    }

    let path = format!("/projects/{}", pid(&w));
    assert_eq!(
        w.matrix(Method::GET, &path, None).await,
        [200, 200, 200, 404, 404]
    );
    let foreign = w.get(&path, &w.other.token).await;
    let missing = w
        .get(&format!("/projects/{}", Uuid::new_v4()), &w.owner.token)
        .await;
    assert_eq!(
        foreign.json(),
        missing.json(),
        "a foreign project looks like a missing one"
    );
    assert_eq!(
        w.get("/projects/not-a-uuid", &w.owner.token).await.status,
        400
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn patch_is_admin_only_and_archives(pool: PgPool) {
    let w = world(pool).await;
    let path = format!("/projects/{}", pid(&w));
    let body = Some(json!({ "name": "Renamed" }));
    assert_eq!(
        w.matrix(Method::PATCH, &path, body).await,
        [200, 200, 403, 404, 404]
    );
    let res = w
        .patch(&path, &w.admin.token, json!({ "isArchived": true, "status": "inactive", "description": "hi", "domain": "x.example.com" }))
        .await;
    let d = data(&res);
    assert_eq!(
        (
            d["name"].clone(),
            d["isArchived"].clone(),
            d["status"].clone(),
            d["domain"].clone()
        ),
        (
            json!("Renamed"),
            json!(true),
            json!("inactive"),
            json!("x.example.com")
        )
    );
    let cleared = w
        .patch(&path, &w.owner.token, json!({ "domain": "" }))
        .await;
    assert!(data(&cleared)["domain"].is_null());
    for bad in [
        json!({ "name": "" }),
        json!({ "slug": "A_B" }),
        json!({ "status": "gone" }),
        json!({ "domain": "nodot" }),
        json!({ "isArchived": "yes" }),
    ] {
        assert_eq!(w.patch(&path, &w.owner.token, bad).await.status, 422);
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn a_patch_whose_mail_half_conflicts_rolls_back_the_project_half(pool: PgPool) {
    let w = world(pool).await;
    let other = format!("/projects/{}", w.foreign.id);
    let r = w
        .patch(
            &other,
            &w.other.token,
            json!({ "smtpEmail": "taken@x.example.com" }),
        )
        .await;
    assert_eq!(r.status, 200);
    let path = format!("/projects/{}", pid(&w));
    let res = w
        .patch(
            &path,
            &w.owner.token,
            json!({ "name": "Changed", "smtpEmail": "TAKEN@x.example.com" }),
        )
        .await;
    assert_eq!(
        (res.status.as_u16(), res.error_code().as_str()),
        (409, "CONFLICT")
    );
    let slug_clash = w
        .patch(
            &path,
            &w.owner.token,
            json!({ "name": "Changed", "slug": "globex" }),
        )
        .await;
    assert_eq!(slug_clash.status, 409);
    let name = data(&w.get(&path, &w.owner.token).await)["name"].clone();
    assert_eq!(
        name,
        json!("Project acme"),
        "the name change was rolled back with the mailbox clash"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn delete_is_owner_only_and_removes_attachment_files(pool: PgPool) {
    use dp_storage::{FsStorage, PutMeta, Storage};
    let dir = std::env::temp_dir().join(format!("sd-r21-{}", Uuid::new_v4()));
    let mut cfg = test_config();
    cfg.storage_dir = dir.clone();
    let w = world_with(TestApp::with_config(pool, cfg).await).await;
    let path = format!("/projects/{}", pid(&w));

    // A stored file referenced by an attachment row of this project.
    let store = FsStorage::new(&dir, 1 << 20).await.unwrap();
    let obj = store.put(&b"hello"[..], PutMeta::default()).await.unwrap();
    sqlx::query("INSERT INTO clients (id, project_id, email) VALUES ($1, $2, 'c@example.com')")
        .bind(dp_db::new_id())
        .bind(w.project.id)
        .execute(&w.t.pool)
        .await
        .unwrap();
    let ticket = dp_db::new_id();
    sqlx::query(
        "INSERT INTO tickets (id, project_id, ticket_number, client_id, subject)
         SELECT $1, $2, 1, id, 'subject' FROM clients WHERE project_id = $2",
    )
    .bind(ticket)
    .bind(w.project.id)
    .execute(&w.t.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO attachments (id, project_id, ticket_id, filename, mime_type, size_bytes, storage_key)
         VALUES ($1, $2, $3, 'a.txt', 'text/plain', 5, $4)",
    )
    .bind(dp_db::new_id())
    .bind(w.project.id)
    .bind(ticket)
    .bind(&obj.key)
    .execute(&w.t.pool)
    .await
    .unwrap();

    for (who, want) in [
        (&w.admin, 403),
        (&w.agent, 403),
        (&w.other, 404),
        (&w.root, 404),
    ] {
        assert_eq!(w.delete(&path, &who.token).await.status, want);
    }
    assert!(
        store.exists(&obj.key).await.unwrap(),
        "refused deletes keep the file"
    );

    let res = w.delete(&path, &w.owner.token).await;
    assert_eq!(res.status, 200);
    assert_eq!(data(&res), json!({ "success": true, "id": w.project.id }));
    assert_eq!(w.get(&path, &w.owner.token).await.status, 404);
    assert!(
        !store.exists(&obj.key).await.unwrap(),
        "the file went with the project"
    );
    // The other tenant is untouched.
    assert_eq!(
        w.get(&format!("/projects/{}", w.foreign.id), &w.other.token)
            .await
            .status,
        200
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn regenerate_key_rotates_the_hash(pool: PgPool) {
    let w = world(pool).await;
    let path = format!("/projects/{}/regenerate-api-key", pid(&w));
    let body = Some(json!({}));
    let hash_of = |w: &W| {
        let (pool, id) = (w.t.pool.clone(), w.project.id);
        async move {
            sqlx::query_scalar::<_, String>("SELECT api_key_hash FROM projects WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    let before = hash_of(&w).await;
    assert_eq!(
        w.matrix(Method::POST, &path, body).await,
        [200, 200, 403, 404, 404]
    );
    let res = w.post(&path, &w.owner.token, json!({})).await;
    let d = data(&res);
    let key = d["apiKey"].as_str().unwrap();
    let after = hash_of(&w).await;
    assert_ne!(before, after);
    assert_eq!(after, hash_api_key(key));
    assert_eq!(d["id"], json!(w.project.id));
    let got = w
        .get(&format!("/projects/{}", pid(&w)), &w.owner.token)
        .await;
    assert_eq!(data(&got)["apiKeyPrefix"], key[..12]);
    assert!(!got.text().contains(key));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn mail_passwords_are_encrypted_write_only_and_never_returned(pool: PgPool) {
    let w = world(pool).await;
    let path = format!("/projects/{}", pid(&w));
    let (sp, ip) = ("smtp-secret-Pw1", "imap-secret-Pw2");
    let res = w
        .patch(
            &path,
            &w.admin.token,
            json!({
                "smtpEmail": "Support@acme.example.com", "smtpHost": "smtp.example.com", "smtpPort": 465,
                "smtpUsername": "mailer", "smtpPassword": sp,
                "imapHost": "imap.example.com", "imapPort": 993, "imapUsername": "mailer",
                "imapPassword": ip, "imapFolder": "Support", "replyToEmail": "reply@acme.example.com",
            }),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    let d = data(&res);
    assert_eq!(
        (d["hasSmtpPassword"].clone(), d["hasImapPassword"].clone()),
        (json!(true), json!(true))
    );
    assert_eq!(d["smtpEmail"], "support@acme.example.com");
    assert_eq!(
        (d["imapFolder"].clone(), d["smtpHost"].clone()),
        (json!("Support"), json!("smtp.example.com"))
    );

    // Every response that can show the project is free of secrets.
    for p in [
        path.clone(),
        "/projects".into(),
        format!("{path}/credentials"),
        format!("{path}/members"),
    ] {
        for who in [&w.owner, &w.admin, &w.agent] {
            let r = w.get(&p, &who.token).await;
            if r.status == 200 {
                assert_no_secrets(&r, &[sp, ip]);
            }
        }
    }
    assert_no_secrets(&res, &[sp, ip]);
    let creds = w.get(&format!("{path}/credentials"), &w.admin.token).await;
    assert_eq!(
        data(&creds),
        json!({
            "email": "support@acme.example.com", "username": "mailer", "smtpHost": "smtp.example.com",
            "smtpPort": 465, "hasSmtpPassword": true, "imapHost": "imap.example.com", "imapPort": 993,
            "imapUsername": "mailer", "imapFolder": "Support", "hasImapPassword": true
        })
    );

    // At rest: ciphertext bound to project and field, opens only with its own aad.
    let (s_enc, i_enc): (String, String) = sqlx::query_as(
        "SELECT smtp_password_encrypted, imap_password_encrypted FROM projects WHERE id = $1",
    )
    .bind(w.project.id)
    .fetch_one(&w.t.pool)
    .await
    .unwrap();
    assert!(s_enc.starts_with("v1:") && !s_enc.contains(sp) && !i_enc.contains(ip));
    let aad = |k| api::routes::project_mail::password_aad(w.project.id, k);
    assert_eq!(w.t.state.secrets.open(&s_enc, &aad("smtp")).unwrap(), sp);
    assert_eq!(w.t.state.secrets.open(&i_enc, &aad("imap")).unwrap(), ip);
    assert!(w.t.state.secrets.open(&s_enc, &aad("imap")).is_err());

    // Omitted = unchanged; "" = cleared.
    let r = w
        .patch(&path, &w.owner.token, json!({ "name": "Still set" }))
        .await;
    assert_eq!(data(&r)["hasSmtpPassword"], true);
    let r = w
        .patch(&path, &w.owner.token, json!({ "smtpPassword": "" }))
        .await;
    assert_eq!(
        (
            data(&r)["hasSmtpPassword"].clone(),
            data(&r)["hasImapPassword"].clone()
        ),
        (json!(false), json!(true))
    );

    // Credentials: admin only.
    let cpath = format!("{path}/credentials");
    assert_eq!(
        w.matrix(Method::GET, &cpath, None).await,
        [200, 200, 403, 404, 404]
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn changing_host_port_or_user_requires_the_password_again(pool: PgPool) {
    let w = world(pool).await;
    let path = format!("/projects/{}", pid(&w));
    let set = json!({
        "smtpHost": "smtp.example.com", "smtpPort": 587, "smtpUsername": "u", "smtpPassword": "pw-one",
        "imapHost": "imap.example.com", "imapPort": 993, "imapUsername": "u", "imapPassword": "pw-two",
    });
    assert_eq!(w.patch(&path, &w.owner.token, set).await.status, 200);

    for (change, field) in [
        (json!({ "smtpHost": "evil.example.net" }), "smtpPassword"),
        (json!({ "smtpPort": 25 }), "smtpPassword"),
        (json!({ "smtpUsername": "someone" }), "smtpPassword"),
        (json!({ "imapHost": "evil.example.net" }), "imapPassword"),
        (json!({ "imapPort": 143 }), "imapPassword"),
        (json!({ "imapUsername": "someone" }), "imapPassword"),
    ] {
        let res = w.patch(&path, &w.owner.token, change.clone()).await;
        assert_eq!(res.status, 422, "{change}");
        assert!(
            res.json()["error"]["details"].get(field).is_some(),
            "{change}"
        );
    }
    // Same values, or a new password, or clearing it, are fine.
    assert_eq!(
        w.patch(
            &path,
            &w.owner.token,
            json!({ "smtpHost": "SMTP.example.com", "smtpPort": 587, "smtpUsername": "u" })
        )
        .await
        .status,
        200
    );
    let r = w
        .patch(
            &path,
            &w.owner.token,
            json!({ "smtpHost": "smtp2.example.com", "smtpPassword": "pw-new" }),
        )
        .await;
    assert_eq!(r.status, 200);
    let r = w
        .patch(
            &path,
            &w.owner.token,
            json!({ "imapHost": "imap2.example.com", "imapPassword": "" }),
        )
        .await;
    assert_eq!(
        (r.status.as_u16(), data(&r)["hasImapPassword"].clone()),
        (200, json!(false))
    );
    // With no stored password nothing needs re-entering.
    assert_eq!(
        w.patch(
            &path,
            &w.owner.token,
            json!({ "imapHost": "imap3.example.com" })
        )
        .await
        .status,
        200
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn private_mail_hosts_are_rejected_unless_allowed(pool: PgPool) {
    let w = world(pool).await;
    let path = format!("/projects/{}", pid(&w));
    for host in [
        "127.0.0.1",
        "localhost",
        "10.1.2.3",
        "169.254.169.254",
        "::1",
        "2130706433",
        "db.internal",
    ] {
        for field in ["smtpHost", "imapHost"] {
            let res = w.patch(&path, &w.owner.token, json!({ field: host })).await;
            assert_eq!(res.status, 422, "{field}={host}");
            assert!(res.json()["error"]["details"].get(field).is_some());
        }
        let res = w
            .post(
                "/projects",
                &w.owner.token,
                json!({ "name": "n", "slug": "ssrf-test", "smtpHost": host }),
            )
            .await;
        assert_eq!(res.status, 422, "create with {host}");
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn the_allow_flag_permits_local_mail_servers(pool: PgPool) {
    let t = TestApp::with_config(pool, config_allowing_private_hosts()).await;
    let w = world_with(t).await;
    let path = format!("/projects/{}", pid(&w));
    let res = w
        .patch(
            &path,
            &w.owner.token,
            json!({ "smtpHost": "127.0.0.1", "imapHost": "localhost" }),
        )
        .await;
    assert_eq!(res.status, 200, "{}", res.text());
    assert_eq!(data(&res)["smtpHost"], "127.0.0.1");
    let bad = w
        .patch(&path, &w.owner.token, json!({ "smtpHost": "a b" }))
        .await;
    assert_eq!(bad.status, 422, "syntax is still checked");
}
