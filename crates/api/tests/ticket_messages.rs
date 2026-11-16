mod common;
mod tickets_fixture;

use std::sync::Arc;

use api::mail_out::{MailError, RecordingMailer, run_outbox_once};
use axum::http::Method;
use serde_json::json;
use sqlx::PgPool;
use tickets_fixture::*;

#[sqlx::test(migrations = "../db/migrations")]
async fn reply_happy_path_mails_after_commit(pool: PgPool) {
    let e = Env::new(pool).await;
    let id = e.ticket_id(&e.owner, "Reply ticket").await;
    let r = e.reply(&id, &e.agent, j("We are on it")).await;
    assert_eq!(r.status, 201, "{}", r.text());
    let m = &r.json()["data"];
    assert_eq!(m["senderType"], "agent");
    assert_eq!(m["messageType"], "reply");
    assert_eq!(m["isInternal"], false);
    assert_eq!(m["sender"]["id"], e.agent_id.to_string());
    // Queued with the message; the worker sends it.
    assert!(e.mailer.sent.lock().unwrap().is_empty());
    let (pending, kind): (String, String) = sqlx::query_as(
        "SELECT l.status, o.kind FROM email_logs l JOIN mail_outbox o ON o.email_log_id = l.id",
    )
    .fetch_one(&e.t.pool)
    .await
    .unwrap();
    assert_eq!((pending.as_str(), kind.as_str()), ("pending", "reply"));
    assert_eq!(run_outbox_once(&e.t.state).await.unwrap(), 1);
    let sent = e.mailer.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].body, "We are on it");
    assert_eq!(sent[0].ticket_id.to_string(), id);
    assert_eq!(sent[0].sent_by, Some(e.agent_id));
    let stored: Option<String> =
        sqlx::query_scalar("SELECT email_message_id FROM messages WHERE id = $1")
            .bind(uuid::Uuid::parse_str(m["id"].as_str().unwrap()).unwrap())
            .fetch_one(&e.t.pool)
            .await
            .unwrap();
    assert_eq!(stored.as_deref(), Some("recorded-1@test.invalid"));
    // thread: newest first
    let list = e.get(&format!("/tickets/{id}/messages"), &e.owner).await;
    let data = list.json()["data"].clone();
    assert_eq!(data[0]["body"], "We are on it");
    assert_eq!(data[1]["senderType"], "client");
    assert_eq!(list.json()["pagination"]["total"], 2);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn send_email_false_and_internal_notes_are_never_mailed(pool: PgPool) {
    let e = Env::new(pool).await;
    let id = e.ticket_id(&e.owner, "Mail rules").await;
    assert_eq!(
        e.reply(&id, &e.owner, json!({"body":"quiet","sendEmail":false}))
            .await
            .status,
        201
    );
    let n = e
        .reply(
            &id,
            &e.owner,
            json!({"body":"internal only","isInternal":true}),
        )
        .await;
    assert_eq!(n.status, 201);
    assert_eq!(n.json()["data"]["messageType"], "note");
    assert_eq!(n.json()["data"]["isInternal"], true);
    // even an explicit sendEmail=true cannot mail a note
    e.reply(
        &id,
        &e.owner,
        json!({"body":"still internal","isInternal":true,"sendEmail":true}),
    )
    .await;
    run_outbox_once(&e.t.state).await.unwrap();
    assert!(e.mailer.sent.lock().unwrap().is_empty());
    assert_eq!(count(&e, "mail_outbox").await, 0);
}

async fn count(e: &Env, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(&e.t.pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn mail_failure_is_recorded_not_surfaced(pool: PgPool) {
    let mailer = Arc::new(RecordingMailer::default());
    *mailer.fail_next.lock().unwrap() = Some(MailError::Internal("smtp down".into()));
    let e = Env::with_mailer(pool, mailer).await;
    let id = e.ticket_id(&e.owner, "Failing mail").await;
    let r = e.reply(&id, &e.owner, j("hello")).await;
    assert_eq!(r.status, 201);
    // message was saved
    assert_eq!(
        e.get(&format!("/tickets/{id}/messages"), &e.owner)
            .await
            .json()["pagination"]["total"],
        2
    );
    // First attempt fails: the job is retried, the log stays pending.
    run_outbox_once(&e.t.state).await.unwrap();
    let (status, attempts, err): (String, i32, Option<String>) =
        sqlx::query_as("SELECT status, attempts, last_error FROM mail_outbox")
            .fetch_one(&e.t.pool)
            .await
            .unwrap();
    assert_eq!((status.as_str(), attempts), ("pending", 1));
    assert_eq!(err.as_deref(), Some("smtp down"));
    let log: String = sqlx::query_scalar("SELECT status FROM email_logs")
        .fetch_one(&e.t.pool)
        .await
        .unwrap();
    assert_eq!(log, "pending");
    // Not due yet: nothing is sent early.
    assert_eq!(run_outbox_once(&e.t.state).await.unwrap(), 0);
    // Due again: the retry succeeds and the thread headers are stored.
    sqlx::query("UPDATE mail_outbox SET next_attempt_at = now()")
        .execute(&e.t.pool)
        .await
        .unwrap();
    assert_eq!(run_outbox_once(&e.t.state).await.unwrap(), 1);
    let (log, smtp_id): (String, Option<String>) =
        sqlx::query_as("SELECT status, smtp_message_id FROM email_logs")
            .fetch_one(&e.t.pool)
            .await
            .unwrap();
    assert_eq!(log, "sent");
    assert_eq!(smtp_id.as_deref(), Some("recorded-1@test.invalid"));
    let stored: Option<String> =
        sqlx::query_scalar("SELECT email_message_id FROM messages WHERE body = 'hello'")
            .fetch_one(&e.t.pool)
            .await
            .unwrap();
    assert_eq!(stored.as_deref(), Some("recorded-1@test.invalid"));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn unconfigured_smtp_gives_up_the_reply_job_at_once(pool: PgPool) {
    let mailer = Arc::new(RecordingMailer::default());
    *mailer.fail_next.lock().unwrap() = Some(MailError::NotConfigured);
    let e = Env::with_mailer(pool, mailer).await;
    let id = e.ticket_id(&e.owner, "No smtp").await;
    assert_eq!(e.reply(&id, &e.owner, j("hello")).await.status, 201);
    run_outbox_once(&e.t.state).await.unwrap();
    let (job, log, err): (String, String, Option<String>) = sqlx::query_as(
        "SELECT o.status, l.status, l.error FROM mail_outbox o JOIN email_logs l ON l.id = o.email_log_id",
    )
    .fetch_one(&e.t.pool)
    .await
    .unwrap();
    assert_eq!((job.as_str(), log.as_str()), ("dead", "failed"));
    assert_eq!(err.as_deref(), Some("SMTP not configured"));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn message_validation_roles_and_isolation(pool: PgPool) {
    let e = Env::new(pool).await;
    let id = e.ticket_id(&e.owner, "Validation").await;
    let path = format!("/tickets/{id}/messages");
    for body in [
        json!({}),
        json!({"body":"   "}),
        json!({"body":"x".repeat(50_001)}),
        json!({"body":"ok","attachmentIds":[uuid::Uuid::new_v4()]}),
        json!({"body":"ok","attachmentIds":vec![uuid::Uuid::new_v4(); 21]}),
    ] {
        let r = e.post(&path, &e.agent, body.clone()).await;
        assert_eq!(r.status, 422, "{body}");
    }
    assert_eq!(
        e.post(&path, &e.agent, json!({"body":"x","isInternal":"yes"}))
            .await
            .status,
        422
    );
    assert_eq!(
        e.call(Method::POST, &path, None, Some(j("x"))).await.status,
        401
    );
    assert_eq!(e.call(Method::GET, &path, None, None).await.status, 401);
    let out = e.reply(&id, &e.outsider, j("x")).await;
    assert_eq!(out.status, 404);
    assert_eq!(e.get(&path, &e.outsider).await.status, 404);
    // senderType in the body is ignored: always an agent message
    let r = e
        .post(&path, &e.agent, json!({"body":"hi","senderType":"client"}))
        .await;
    assert_eq!(r.json()["data"]["senderType"], "agent");
    run_outbox_once(&e.t.state).await.unwrap();
    assert!(e.mailer.sent.lock().unwrap().len() == 1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn message_pagination_is_bounded(pool: PgPool) {
    let e = Env::new(pool).await;
    let id = e.ticket_id(&e.owner, "Many messages").await;
    for i in 0..4 {
        e.reply(
            &id,
            &e.owner,
            json!({"body":format!("m{i}"),"sendEmail":false}),
        )
        .await;
    }
    let path = format!("/tickets/{id}/messages");
    let r = e.get(&format!("{path}?limit=2&offset=1"), &e.owner).await;
    assert_eq!(r.json()["data"].as_array().unwrap().len(), 2);
    assert_eq!(r.json()["data"][0]["body"], "m2");
    assert_eq!(r.json()["pagination"]["hasMore"], true);
    assert_eq!(
        e.get(&format!("{path}?limit=9999"), &e.owner).await.json()["pagination"]["limit"],
        200
    );
    assert_eq!(
        e.get(&format!("{path}?limit=0"), &e.owner).await.status,
        422
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn first_public_reply_sets_first_response_but_notes_do_not(pool: PgPool) {
    let e = Env::new(pool).await;
    let id = e.ticket_id(&e.owner, "Timing").await;
    let uid = uuid::Uuid::parse_str(&id).unwrap();
    let frt = || async {
        sqlx::query_scalar::<_, Option<chrono::DateTime<chrono::Utc>>>(
            "SELECT first_response_at FROM tickets WHERE id = $1",
        )
        .bind(uid)
        .fetch_one(&e.t.pool)
        .await
        .unwrap()
    };
    e.reply(&id, &e.owner, json!({"body":"note","isInternal":true}))
        .await;
    assert!(frt().await.is_none());
    e.reply(&id, &e.owner, j("public")).await;
    assert!(frt().await.is_some());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn unread_list_read_and_unread(pool: PgPool) {
    let e = Env::new(pool).await;
    let id = e.ticket_id(&e.owner, "Unread flow").await;
    for path in ["/unread/", "/unread"] {
        let r = e.get(path, &e.agent).await;
        assert_eq!(r.status, 200, "{path}");
        assert_eq!(r.json()["data"].as_array().unwrap().len(), 1);
    }
    let p = e.project.id;
    assert_eq!(
        e.get(&format!("/unread/?projectId={p}"), &e.agent)
            .await
            .json()["data"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        e.get(
            &format!("/unread/?projectId={}", uuid::Uuid::new_v4()),
            &e.agent
        )
        .await
        .json()["data"],
        json!([])
    );
    assert_eq!(e.get("/unread/?projectId=x", &e.agent).await.status, 422);
    assert_eq!(
        e.get("/unread/", &e.outsider).await.json()["data"],
        json!([])
    );
    assert_eq!(
        e.call(Method::GET, "/unread/", None, None).await.status,
        401
    );

    let path = format!("/unread/ticket/{id}");
    assert_eq!(e.delete(&path, &e.outsider).await.status, 404);
    assert_eq!(e.call(Method::DELETE, &path, None, None).await.status, 401);
    assert_eq!(
        e.get("/unread/", &e.agent).await.json()["data"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let r = e.delete(&path, &e.agent).await;
    assert_eq!(r.json()["data"], json!({"deleted": 1}));
    assert_eq!(
        e.delete(&path, &e.agent).await.json()["data"],
        json!({"deleted": 0})
    );
    assert_eq!(e.get("/unread/", &e.owner).await.json()["data"], json!([]));

    assert_eq!(e.post(&path, &e.outsider, json!({})).await.status, 404);
    assert_eq!(
        e.post(&path, &e.agent, json!({})).await.json()["data"],
        json!({"created": true})
    );
    assert_eq!(
        e.post(&path, &e.agent, json!({})).await.json()["data"],
        json!({"created": false})
    );
    assert_eq!(
        e.get("/unread/", &e.owner).await.json()["data"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
