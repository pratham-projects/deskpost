mod common;
mod tickets_fixture;

use axum::body::Body;
use axum::http::header;
use serde_json::json;
use sqlx::PgPool;
use tickets_fixture::*;

fn fields(e: &Env) -> Vec<(&'static str, String)> {
    [
        ("projectId", e.project.id.to_string()),
        ("clientEmail", "Visitor@Example.com".into()),
        ("clientName", "Vera Visitor".into()),
        ("subject", "Widget question".into()),
        ("description", "How do I reset?".into()),
    ]
    .into()
}

async fn submit(e: &Env, key: Option<&str>, f: &[(&str, String)]) -> common::Resp {
    let f: Vec<(&str, &str)> = f.iter().map(|(k, v)| (*k, v.as_str())).collect();
    e.form("/tickets", None, key, &f).await
}

#[sqlx::test(migrations = "../db/migrations")]
async fn widget_creates_client_ticket_message_without_cookies(pool: PgPool) {
    let e = Env::new(pool).await;
    let r = submit(&e, Some(KEY), &fields(&e)).await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert!(r.set_cookie().is_none());
    let t = &r.json()["data"];
    assert_eq!(t["ticketNumber"], 1);
    assert_eq!(t["status"], "pending");
    assert_eq!(t["client"]["email"], "visitor@example.com");
    assert_eq!(t["client"]["name"], "Vera Visitor");
    assert_eq!(t["createdByUserId"], json!(null));
    let id = t["id"].as_str().unwrap();
    let m = e.get(&format!("/tickets/{id}/messages"), &e.owner).await;
    assert_eq!(m.json()["data"][0]["senderType"], "client");
    assert_eq!(m.json()["data"][0]["body"], "How do I reset?");
    assert_eq!(
        e.get("/unread/", &e.owner).await.json()["data"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let (source, creator): (String, Option<uuid::Uuid>) =
        sqlx::query_as("SELECT source, created_by_user_id FROM tickets")
            .fetch_one(&e.t.pool)
            .await
            .unwrap();
    assert_eq!((source.as_str(), creator), ("web", None));
    // same client email reuses the client row
    let r2 = submit(&e, Some(KEY), &fields(&e)).await;
    assert_eq!(r2.json()["data"]["clientId"], t["clientId"]);
    assert_eq!(r2.json()["data"]["ticketNumber"], 2);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn widget_key_abuse_cases(pool: PgPool) {
    let e = Env::new(pool).await;
    let f = fields(&e);
    for key in [
        None,
        Some(""),
        Some("sdk_wrong"),
        Some("sdk_test_widget_key "),
    ] {
        let r = submit(&e, key, &f).await;
        assert_eq!(
            (r.status.as_u16(), r.error_code().as_str()),
            (401, "TOKEN_INVALID"),
            "{key:?}"
        );
    }
    // a valid key of project A for project B's id is rejected the same way
    let other = e.t.project(e.owner_id, "other").await;
    let mut f2 = fields(&e);
    f2[0].1 = other.id.to_string();
    assert_eq!(submit(&e, Some(KEY), &f2).await.status, 401);
    // unknown project id: indistinguishable from a bad key
    f2[0].1 = uuid::Uuid::new_v4().to_string();
    let r = submit(&e, Some(KEY), &f2).await;
    assert_eq!(r.status, 401);
    // a bearer token is no substitute for the key
    let ff: Vec<(&str, &str)> = f.iter().map(|(k, v)| (*k, v.as_str())).collect();
    assert_eq!(
        e.form("/tickets", Some(&e.owner), None, &ff).await.status,
        401
    );
    // inactive project
    sqlx::query("UPDATE projects SET status = 'inactive' WHERE id = $1")
        .bind(e.project.id)
        .execute(&e.t.pool)
        .await
        .unwrap();
    assert_eq!(submit(&e, Some(KEY), &f).await.status, 401);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM tickets")
        .fetch_one(&e.t.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn widget_validation_and_content_type(pool: PgPool) {
    let e = Env::new(pool).await;
    let mut f = fields(&e);
    f[1].1 = "nope".into();
    f[3].1 = "abc".into();
    f.retain(|(k, _)| *k != "description");
    let r = submit(&e, Some(KEY), &f).await;
    assert_eq!(r.status, 422);
    let d = &r.json()["error"]["details"];
    assert!(
        d["clientEmail"].is_string() && d["subject"].is_string() && d["description"].is_string()
    );
    // JSON body is not accepted by the widget
    let req = e.t.post("/tickets").header("x-project-key", KEY);
    let r =
        e.t.send_json(req, json!({"subject":"Widget question"}))
            .await;
    assert_eq!(r.status, 415);
    // garbage multipart
    let req = e.t.post("/tickets").header("x-project-key", KEY).header(
        header::CONTENT_TYPE,
        "multipart/form-data; boundary=XBOUNDARYX",
    );
    let r =
        e.t.send_body(req, Body::from("--XBOUNDARYX\r\nnot a part"))
            .await;
    assert_eq!(r.status, 400);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn widget_body_is_limited_and_files_are_skipped(pool: PgPool) {
    let e = Env::new(pool).await;
    let mut f = fields(&e);
    f[4].1 = "x".repeat(70 * 1024);
    let r = submit(&e, Some(KEY), &f).await;
    assert_eq!(
        (r.status.as_u16(), r.error_code().as_str()),
        (413, "PAYLOAD_TOO_LARGE")
    );
    // unknown parts (files) do not break the form
    let boundary = "XBOUNDARYX";
    let mut body = String::new();
    for (k, v) in fields(&e) {
        body.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n"
        ));
    }
    body.push_str(&format!("--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\nhi\r\n--{boundary}--\r\n"));
    let req = e.t.post("/tickets").header("x-project-key", KEY).header(
        header::CONTENT_TYPE,
        &format!("multipart/form-data; boundary={boundary}"),
    );
    assert_eq!(e.t.send_body(req, Body::from(body)).await.status, 201);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn widget_is_rate_limited_per_ip(pool: PgPool) {
    let e = Env::new(pool).await;
    let f = fields(&e);
    // Public quota is 20/min; bad keys count too, so key guessing is throttled.
    for _ in 0..20 {
        assert_eq!(submit(&e, Some("sdk_guess"), &f).await.status, 401);
    }
    let r = submit(&e, Some(KEY), &f).await;
    assert_eq!(r.status, 429);
    assert!(r.header("retry-after").is_some());
    // another client address is unaffected
    let ff: Vec<(&str, &str)> = f.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (ct, body) = multipart(&ff);
    let req =
        e.t.post("/tickets")
            .header("x-project-key", KEY)
            .header(header::CONTENT_TYPE, &ct)
            .peer("198.51.100.7");
    assert_eq!(e.t.send_body(req, Body::from(body)).await.status, 201);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn widget_response_and_mail_never_expose_internal_notes(pool: PgPool) {
    let e = Env::new(pool).await;
    let r = submit(&e, Some(KEY), &fields(&e)).await;
    let id = r.json()["data"]["id"].as_str().unwrap().to_string();
    e.reply(
        &id,
        &e.owner,
        json!({"body":"TOP-SECRET-NOTE","isInternal":true}),
    )
    .await;
    e.reply(&id, &e.owner, j("public reply")).await;
    assert!(!r.text().contains("TOP-SECRET-NOTE"));
    // the only customer-facing paths: the mail and the widget response
    for m in e.mailer.sent.lock().unwrap().iter() {
        assert!(!m.body.contains("TOP-SECRET-NOTE"));
    }
    assert_eq!(e.mailer.sent.lock().unwrap().len(), 1);
    // and the public endpoint cannot read threads at all
    let g =
        e.t.send(
            e.t.get(&format!("/tickets/{id}/messages"))
                .header("x-project-key", KEY),
        )
        .await;
    assert_eq!(g.status, 401);
}
