mod common;
mod mailkit;

use axum::body::Body;
use axum::http::Method;
use common::*;
use mailkit::*;
use sd_db::Role;
use sd_storage::Disposition;
use serde_json::{Value, json};
use sqlx::PgPool;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n-not-really-a-png";

impl Kit {
    async fn put(&self, tok: &str, ticket: sd_db::Id, name: &str, ct: &str, bytes: &[u8]) -> Resp {
        let req = self
            .t
            .req(Method::PUT, &format!("/attachments/{ticket}"))
            .bearer(tok)
            .header("content-type", ct)
            .header("x-filename", name);
        self.t.send_body(req, Body::from(bytes.to_vec())).await
    }

    async fn multipart(
        &self,
        tok: &str,
        ticket: sd_db::Id,
        files: &[(&str, &str, &[u8])],
        fields: &[(&str, &str)],
    ) -> Resp {
        let b = "XBOUNDARYX";
        let mut body: Vec<u8> = Vec::new();
        for (k, v) in fields {
            body.extend(
                format!("--{b}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n")
                    .as_bytes(),
            );
        }
        for (name, ct, bytes) in files {
            body.extend(format!("--{b}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"{name}\"\r\nContent-Type: {ct}\r\n\r\n").as_bytes());
            body.extend(*bytes);
            body.extend(b"\r\n");
        }
        body.extend(format!("--{b}--\r\n").as_bytes());
        let req = self
            .t
            .req(Method::POST, &format!("/attachments/{ticket}/multiple"))
            .bearer(tok)
            .header(
                "content-type",
                &format!("multipart/form-data; boundary={b}"),
            );
        self.t.send_body(req, Body::from(body)).await
    }

    async fn get_file(&self, tok: &str, id: &str) -> Resp {
        self.t
            .send(self.t.get(&format!("/attachments/file/{id}")).bearer(tok))
            .await
    }
}

fn id_of(r: &Resp) -> String {
    r.json()["data"]["attachmentId"]
        .as_str()
        .unwrap()
        .to_string()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn upload_list_download_round_trip(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (_, _, ticket, s) = k.tenant("owner@example.com", "acme").await;
    let r = k
        .put(&s.access_token, ticket.id, "pic.png", "image/png", PNG)
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let id = id_of(&r);
    assert!(r.json()["data"]["messageId"].is_string());

    // The upload became an agent message carrying the file.
    let msg: (String, String) =
        sqlx::query_as("SELECT body, sender_type FROM messages WHERE id = $1::uuid")
            .bind(r.json()["data"]["messageId"].as_str().unwrap())
            .fetch_one(&k.t.pool)
            .await
            .unwrap();
    assert_eq!(msg, ("Sent an attachment: pic.png".into(), "agent".into()));

    let list =
        k.t.send(
            k.t.get(&format!("/attachments/ticket/{}", ticket.id))
                .bearer(&s.access_token),
        )
        .await;
    assert_eq!(list.status, 200);
    let data = list.json()["data"].clone();
    assert_eq!(data[0]["filename"], "pic.png");
    assert_eq!(data[0]["sizeBytes"], PNG.len());
    assert!(data[0].get("storageKey").is_none() && !list.text().contains("storage"));

    let f = k.get_file(&s.access_token, &id).await;
    assert_eq!(f.status, 200);
    assert_eq!(f.body, PNG);
    assert_eq!(f.header("content-type"), Some("image/png"));
    assert!(
        f.header("content-disposition")
            .unwrap()
            .starts_with("inline;")
    );
    assert_eq!(f.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(f.header("cache-control"), Some("private"));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn only_allowlisted_types_display_inline(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (_, _, ticket, s) = k.tenant("owner@example.com", "acme").await;
    for (ct, inline) in [
        ("text/plain", true),
        ("application/pdf", false),
        ("application/zip", false),
    ] {
        let r = k.put(&s.access_token, ticket.id, "f.bin", ct, b"abc").await;
        let f = k.get_file(&s.access_token, &id_of(&r)).await;
        let disp = f.header("content-disposition").unwrap();
        assert_eq!(disp.starts_with("inline;"), inline, "{ct}: {disp}");
        if !inline {
            assert_eq!(f.header("content-type"), Some("application/octet-stream"));
        }
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn upload_rejects_bad_types_sizes_and_empty(pool: PgPool) {
    let k = Kit::build(pool, false, |c| {
        c.mail.max_file_bytes = 100;
        c.mail.max_request_bytes = 150;
        c.mail.max_files = 2;
    })
    .await;
    let (_, _, ticket, s) = k.tenant("owner@example.com", "acme").await;
    let tok = &s.access_token;
    for ct in [
        "text/html",
        "image/svg+xml",
        "application/x-msdownload",
        "garbage",
        "",
    ] {
        let r = k.put(tok, ticket.id, "x", ct, b"abc").await;
        assert_eq!(
            (r.status.as_u16(), r.error_code().as_str()),
            (415, "UNSUPPORTED_MEDIA_TYPE"),
            "{ct:?}"
        );
    }
    let r = k.put(tok, ticket.id, "x", "text/plain", &[b'a'; 101]).await;
    assert_eq!(
        (r.status.as_u16(), r.error_code().as_str()),
        (413, "PAYLOAD_TOO_LARGE")
    );
    let r = k.put(tok, ticket.id, "x", "text/plain", b"").await;
    assert_eq!(r.status, 400);
    // 3 files > max_files, and 2 x 90 bytes > the request total.
    let files: Vec<(&str, &str, &[u8])> = vec![
        ("a", "text/plain", b"1"),
        ("b", "text/plain", b"2"),
        ("c", "text/plain", b"3"),
    ];
    assert_eq!(k.multipart(tok, ticket.id, &files, &[]).await.status, 413);
    let big = [b'a'; 90];
    let files: Vec<(&str, &str, &[u8])> =
        vec![("a", "text/plain", &big), ("b", "text/plain", &big)];
    assert_eq!(k.multipart(tok, ticket.id, &files, &[]).await.status, 413);
    // Nothing was kept from any refused request.
    assert_eq!(k.count("attachments").await, 0);
    assert_eq!(k.stored_files(), 0);
    assert_eq!(k.count("messages").await, 1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn request_over_the_body_limit_is_413(pool: PgPool) {
    let k = Kit::build(pool, false, |c| c.upload_limit = 1024).await;
    let (_, _, ticket, s) = k.tenant("owner@example.com", "acme").await;
    let r = k
        .put(&s.access_token, ticket.id, "x", "text/plain", &[b'a'; 4096])
        .await;
    assert_eq!(r.status, 413);
    assert_eq!(r.error_code(), "PAYLOAD_TOO_LARGE");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn project_quota_is_enforced_and_cleans_up(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (_, project, ticket, s) = k.tenant("owner@example.com", "acme").await;
    sqlx::query("UPDATE projects SET storage_quota_mb = 0 WHERE id = $1")
        .bind(project.id)
        .execute(&k.t.pool)
        .await
        .unwrap();
    let r = k
        .put(&s.access_token, ticket.id, "a.txt", "text/plain", b"abc")
        .await;
    assert_eq!(r.status, 413);
    assert_eq!(k.count("attachments").await, 0);
    assert_eq!(
        k.stored_files(),
        0,
        "object written before the quota check is removed"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn filenames_are_sanitised_and_headers_safe(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (_, _, ticket, s) = k.tenant("owner@example.com", "acme").await;
    let r = k
        .put(
            &s.access_token,
            ticket.id,
            "../../etc/passwd",
            "text/plain",
            b"x",
        )
        .await;
    let list =
        k.t.send(
            k.t.get(&format!("/attachments/ticket/{}", ticket.id))
                .bearer(&s.access_token),
        )
        .await;
    assert_eq!(list.json()["data"][0]["filename"], "passwd");
    let _ = r;
    let r = k
        .put(
            &s.access_token,
            ticket.id,
            "a\";x=\".html",
            "application/zip",
            b"x",
        )
        .await;
    let f = k.get_file(&s.access_token, &id_of(&r)).await;
    let d = f.header("content-disposition").unwrap();
    assert!(d.starts_with("attachment; filename=\""), "{d}");
    assert!(!d.contains("\";x=\""), "{d}");
    // Nothing on disk is named after the client: keys are generated.
    fn names(p: &std::path::Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
            out.push(e.file_name().to_string_lossy().into_owned());
            if e.path().is_dir() {
                names(&e.path(), out);
            }
        }
    }
    let mut all = vec![];
    names(&k.dir.path().join("files"), &mut all);
    assert!(
        all.iter()
            .all(|n| !n.contains("passwd") && !n.contains("html")),
        "{all:?}"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn tenant_isolation_on_every_route(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (_, _, ticket, s) = k.tenant("owner@example.com", "acme").await;
    let r = k
        .put(&s.access_token, ticket.id, "a.txt", "text/plain", b"abc")
        .await;
    let id = id_of(&r);
    let outsider = k.t.user("out@example.com", Role::Admin).await;
    let _p = k.t.project(outsider.id, "other").await;
    let o = k.t.signin("out@example.com").await.access_token;

    let missing = uuid::Uuid::new_v4();
    for (method, path) in [
        (Method::PUT, format!("/attachments/{}", ticket.id)),
        (Method::GET, format!("/attachments/ticket/{}", ticket.id)),
        (Method::GET, format!("/attachments/file/{id}")),
        (Method::GET, format!("/attachments/file/{id}/url")),
        (Method::DELETE, format!("/attachments/file/{id}")),
    ] {
        let res =
            k.t.send_body(
                k.t.req(method.clone(), &path)
                    .bearer(&o)
                    .header("content-type", "text/plain")
                    .header("x-filename", "x"),
                Body::from("abc"),
            )
            .await;
        assert_eq!(res.status, 404, "{method} {path}");
        let ghost = path
            .replace(&ticket.id.to_string(), &missing.to_string())
            .replace(&id, &missing.to_string());
        let res2 =
            k.t.send_body(
                k.t.req(method.clone(), &ghost)
                    .bearer(&o)
                    .header("content-type", "text/plain")
                    .header("x-filename", "x"),
                Body::from("abc"),
            )
            .await;
        assert_eq!(
            res.json(),
            res2.json(),
            "foreign id looks like a missing id: {method} {path}"
        );
    }
    // Still there, and nothing was added.
    assert_eq!(k.count("attachments").await, 1);
    assert_eq!(k.get_file(&s.access_token, &id).await.status, 200);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn routes_need_a_token(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (_, _, ticket, s) = k.tenant("owner@example.com", "acme").await;
    for (m, p) in [
        (Method::PUT, format!("/attachments/{}", ticket.id)),
        (Method::POST, format!("/attachments/{}/multiple", ticket.id)),
        (Method::GET, format!("/attachments/ticket/{}", ticket.id)),
        (
            Method::GET,
            format!("/attachments/file/{}", uuid::Uuid::new_v4()),
        ),
        (
            Method::GET,
            format!("/attachments/file/{}/url", uuid::Uuid::new_v4()),
        ),
        (
            Method::DELETE,
            format!("/attachments/file/{}", uuid::Uuid::new_v4()),
        ),
    ] {
        let r = k.t.send(k.t.req(m.clone(), &p)).await;
        assert_eq!(
            (r.status.as_u16(), r.error_code().as_str()),
            (401, "TOKEN_MISSING"),
            "{m} {p}"
        );
    }
    assert_eq!(
        k.t.send(
            k.t.get("/attachments/ticket/not-a-uuid")
                .bearer(&s.access_token)
        )
        .await
        .status,
        400
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn signed_urls_work_without_a_bearer_and_cannot_be_abused(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (_, _, ticket, s) = k.tenant("owner@example.com", "acme").await;
    let png = id_of(
        &k.put(&s.access_token, ticket.id, "p.png", "image/png", PNG)
            .await,
    );
    let zip = id_of(
        &k.put(
            &s.access_token,
            ticket.id,
            "z.zip",
            "application/zip",
            b"PK",
        )
        .await,
    );

    let url = |r: &Resp| r.json()["data"]["url"].as_str().unwrap().to_string();
    let r =
        k.t.send(
            k.t.get(&format!("/attachments/file/{png}/url"))
                .bearer(&s.access_token),
        )
        .await;
    assert_eq!(r.status, 200);
    let u = url(&r);
    assert!(
        u.starts_with(&format!("/api/v3/attachments/file/{png}?token=")),
        "{u}"
    );
    let path = u.strip_prefix("/api/v3").unwrap();
    let ok = k.t.send(k.t.get(path)).await;
    assert_eq!((ok.status.as_u16(), ok.body.as_slice()), (200, PNG));
    assert!(
        ok.header("content-disposition")
            .unwrap()
            .starts_with("inline;")
    );

    // Tampered, truncated, someone else's, and another file's token all fail.
    let token = u.split("token=").nth(1).unwrap();
    let flipped = format!(
        "{}{}",
        &token[..token.len() - 1],
        if token.ends_with('0') { '1' } else { '0' }
    );
    for bad in [flipped.as_str(), "garbage", "", &token[..token.len() / 2]] {
        let r =
            k.t.send(k.t.get(&format!("/attachments/file/{png}?token={bad}")))
                .await;
        assert_eq!(r.status, 401, "token {bad:?}");
    }
    assert_eq!(
        k.t.send(k.t.get(&format!("/attachments/file/{zip}?token={token}")))
            .await
            .status,
        401
    );

    // Expired (beyond the leeway).
    let key: String = sqlx::query_scalar("SELECT storage_key FROM attachments WHERE id = $1::uuid")
        .bind(&png)
        .fetch_one(&k.t.pool)
        .await
        .unwrap();
    let now = chrono::Utc::now().timestamp() as u64;
    let old =
        k.t.state
            .store
            .signer
            .sign(&key, Disposition::Inline, now - 120);
    assert_eq!(
        k.t.send(k.t.get(&format!("/attachments/file/{png}?token={old}")))
            .await
            .status,
        401
    );

    // A token that claims "inline" for a download-only type still downloads.
    let zkey: String =
        sqlx::query_scalar("SELECT storage_key FROM attachments WHERE id = $1::uuid")
            .bind(&zip)
            .fetch_one(&k.t.pool)
            .await
            .unwrap();
    let forged =
        k.t.state
            .store
            .signer
            .sign(&zkey, Disposition::Inline, now + 60);
    let f =
        k.t.send(k.t.get(&format!("/attachments/file/{zip}?token={forged}")))
            .await;
    assert_eq!(f.status, 200);
    assert!(
        f.header("content-disposition")
            .unwrap()
            .starts_with("attachment;")
    );
    assert_eq!(
        k.t.send(
            k.t.get(&format!("/attachments/file/{zip}/url"))
                .bearer(&s.access_token)
        )
        .await
        .status,
        200
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn delete_removes_row_and_file_for_uploader_or_admin_only(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (owner, project, ticket, s) = k.tenant("owner@example.com", "acme").await;
    let agent = k.t.user("agent@example.com", Role::Agent).await;
    let other = k.t.user("other@example.com", Role::Agent).await;
    k.t.add_member(owner.id, project.id, agent.id, Role::Agent)
        .await;
    k.t.add_member(owner.id, project.id, other.id, Role::Agent)
        .await;
    let a = k.t.signin("agent@example.com").await.access_token;
    let o = k.t.signin("other@example.com").await.access_token;

    let id = id_of(&k.put(&a, ticket.id, "a.txt", "text/plain", b"abc").await);
    assert_eq!(k.stored_files(), 2, "data + sidecar");
    let del = |tok: String, id: String| {
        let k = &k;
        async move {
            k.t.send(
                k.t.req(Method::DELETE, &format!("/attachments/file/{id}"))
                    .bearer(&tok),
            )
            .await
        }
    };
    assert_eq!(del(o.clone(), id.clone()).await.status, 403);
    assert_eq!(k.count("attachments").await, 1);
    let r = del(a.clone(), id.clone()).await;
    assert_eq!(
        (r.status.as_u16(), r.json()["data"]["success"].clone()),
        (200, json!(true))
    );
    assert_eq!((k.count("attachments").await, k.stored_files()), (0, 0));
    assert_eq!(del(a.clone(), id).await.status, 404);

    let id2 = id_of(&k.put(&a, ticket.id, "b.txt", "text/plain", b"abc").await);
    assert_eq!(
        del(s.access_token.clone(), id2).await.status,
        200,
        "project admin"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn multipart_with_email_sends_attachments_and_threads(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (_, project, ticket, s) = k.tenant("owner@example.com", "acme").await;
    let smtp = sd_mail::testing::FakeSmtp::start_requiring_auth("u", "smtp-secret-pw").await;
    k.configure_mail(project.id, smtp.port(), None).await;

    let files: Vec<(&str, &str, &[u8])> = vec![
        ("one.txt", "text/plain", b"first"),
        ("two.png", "image/png", PNG),
    ];
    let r = k
        .multipart(
            &s.access_token,
            ticket.id,
            &files,
            &[("message", "Here you go"), ("sendEmail", "true")],
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    let ids = r.json()["data"]["attachmentIds"].as_array().unwrap().len();
    assert_eq!(ids, 2);

    let mails = smtp.messages();
    assert_eq!(mails.len(), 1);
    assert_eq!(mails[0].rcpt_to, ["client@example.com"]);
    let parsed = sd_mail::parse_inbound(&mails[0].data, &sd_mail::Limits::default()).unwrap();
    assert_eq!(parsed.text.as_deref().map(str::trim), Some("Here you go"));
    let mut names: Vec<_> = parsed
        .attachments
        .iter()
        .map(|a| a.filename.clone())
        .collect();
    names.sort();
    assert_eq!(names, ["one.txt", "two.png"]);
    assert!(
        parsed
            .subject
            .contains(&format!("[Ticket #{}]", ticket.ticket_number))
    );

    // The stored message carries the Message-ID so a customer reply threads onto it.
    let mid: Option<String> =
        sqlx::query_scalar("SELECT email_message_id FROM messages WHERE body = 'Here you go'")
            .fetch_one(&k.t.pool)
            .await
            .unwrap();
    assert_eq!(mid.as_deref(), Some(parsed.message_id.as_str()));
    let log: (String, Option<String>) = sqlx::query_as("SELECT status, error FROM email_logs")
        .fetch_one(&k.t.pool)
        .await
        .unwrap();
    assert_eq!(log, ("sent".into(), None));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn failed_mail_saves_nothing(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (_, project, ticket, s) = k.tenant("owner@example.com", "acme").await;
    // Not configured at all: 422.
    let r = k
        .multipart(
            &s.access_token,
            ticket.id,
            &[("a.txt", "text/plain", b"x")],
            &[("sendEmail", "true")],
        )
        .await;
    assert_eq!(
        (r.status.as_u16(), r.error_code().as_str()),
        (422, "VALIDATION_ERROR")
    );
    // Server unreachable: 502, and the SMTP password is nowhere in the answer or the log.
    k.configure_mail(project.id, closed_port(), None).await;
    let r = k
        .multipart(
            &s.access_token,
            ticket.id,
            &[("a.txt", "text/plain", b"x")],
            &[("sendEmail", "true")],
        )
        .await;
    assert_eq!(
        (r.status.as_u16(), r.error_code().as_str()),
        (502, "MAIL_DELIVERY_FAILED")
    );
    assert!(!r.text().contains("smtp-secret-pw"));
    assert_eq!(
        (
            k.count("attachments").await,
            k.count("messages").await,
            k.stored_files()
        ),
        (0, 1, 0)
    );
    let log: (String, Option<String>) = sqlx::query_as("SELECT status, error FROM email_logs")
        .fetch_one(&k.t.pool)
        .await
        .unwrap();
    assert_eq!(log.0, "failed");
    assert!(!log.1.unwrap().contains("smtp-secret-pw"));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn upload_without_email_does_not_touch_the_mailer(pool: PgPool) {
    let k = Kit::recording(pool).await;
    let (_, _, ticket, s) = k.tenant("owner@example.com", "acme").await;
    assert_eq!(
        k.put(&s.access_token, ticket.id, "a.txt", "text/plain", b"abc")
            .await
            .status,
        201
    );
    assert!(k.recorder.sent.lock().unwrap().is_empty());
    let req =
        k.t.req(Method::PUT, &format!("/attachments/{}", ticket.id))
            .bearer(&s.access_token)
            .header("content-type", "text/plain")
            .header("x-filename", "b.txt")
            .header("x-send-email", "true")
            .header("x-message", "see attached");
    let r = k.t.send_body(req, Body::from("abc")).await;
    assert_eq!(r.status, 201);
    {
        let sent = k.recorder.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            (
                sent[0].body.as_str(),
                sent[0].attachment_ids.len(),
                sent[0].ticket_id
            ),
            ("see attached", 1, ticket.id)
        );
    }
    let stored: Value = json!(
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT email_message_id FROM messages WHERE body = 'see attached'"
        )
        .fetch_one(&k.t.pool)
        .await
        .unwrap()
    );
    assert_eq!(stored, json!("recorded-1@test.invalid"));
}
