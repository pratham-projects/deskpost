mod common;
mod mailkit;

use api::routes::incoming::sign;
use axum::body::Body;
use axum::http::Method;
use base64::Engine;
use common::*;
use dp_db::{EmailResponseStatus, Role, email_responses};
use mailkit::*;
use serde_json::{Value, json};
use sqlx::PgPool;

impl Kit {
    async fn hook_at(&self, body: &Value, ts: i64) -> Resp {
        let raw = body.to_string();
        let sig = sign(WEBHOOK_SECRET.as_bytes(), ts, raw.as_bytes());
        let req = self
            .t
            .req(Method::POST, "/incoming-email")
            .header("content-type", "application/json")
            .header("x-webhook-timestamp", &ts.to_string())
            .header("x-webhook-signature", &sig);
        self.t.send_body(req, Body::from(raw)).await
    }

    async fn hook(&self, body: Value) -> Resp {
        self.hook_at(&body, chrono::Utc::now().timestamp()).await
    }

    async fn hook_raw(&self, raw: &str, headers: &[(&str, &str)]) -> Resp {
        let mut req = self.t.req(Method::POST, "/incoming-email");
        for (k, v) in headers {
            req = req.header(*k, v);
        }
        self.t.send_body(req, Body::from(raw.to_string())).await
    }

    /// Project "acme" with its mailbox configured (no live SMTP needed for ingest).
    async fn acme(&self) -> (dp_db::users::User, dp_db::projects::Project) {
        let owner = self.t.user("owner@example.com", Role::Agent).await;
        let project = self.t.project(owner.id, "acme").await;
        self.configure_mail(project.id, closed_port(), None).await;
        (owner, project)
    }
}

fn mail(id: &str, subject: &str, body: &str) -> Value {
    json!({ "from": "Ann <Ann@Example.com>", "to": MAILBOX, "subject": subject, "body": body, "messageId": id })
}

async fn tickets(k: &Kit) -> Vec<(i64, String, String, String)> {
    sqlx::query_as("SELECT t.ticket_number, t.subject, t.status, c.email FROM tickets t JOIN clients c ON c.id = t.client_id ORDER BY t.ticket_number")
        .fetch_all(&k.t.pool).await.unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn new_mail_opens_a_ticket(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (owner, project) = k.acme().await;
    email_responses::create(
        &k.t.pool,
        owner.id,
        project.id,
        EmailResponseStatus::Created,
        "",
        "Got it",
        true,
    )
    .await
    .unwrap()
    .unwrap();
    let r = k
        .hook(mail(
            "<m1@mail.example>",
            "Re: Printer broken",
            "It does not print\n\nOn Monday Bob wrote:\n> old stuff",
        ))
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    assert_eq!(r.json()["data"]["created"], true);
    assert_eq!(
        tickets(&k).await,
        [(
            1,
            "Printer broken".into(),
            "pending".into(),
            "ann@example.com".into()
        )]
    );
    let m: (String, String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT body, sender_type, email_message_id, html_body FROM messages")
            .fetch_one(&k.t.pool)
            .await
            .unwrap();
    assert_eq!(
        m,
        (
            "It does not print".into(),
            "client".into(),
            Some("m1@mail.example".into()),
            None
        )
    );
    assert_eq!(k.count("unread_tickets").await, 1);
    let kinds: Vec<String> = sqlx::query_scalar("SELECT kind FROM mail_outbox ORDER BY kind")
        .fetch_all(&k.t.pool)
        .await
        .unwrap();
    // The confirmation for the client and the new-ticket notice for the owner (shouldNotify).
    assert_eq!(kinds, ["created", "new_ticket"], "mail queued");
    let source: String = sqlx::query_scalar("SELECT source FROM tickets")
        .fetch_one(&k.t.pool)
        .await
        .unwrap();
    assert_eq!(source, "email");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn signature_and_timestamp_are_enforced(pool: PgPool) {
    let k = Kit::new(pool).await;
    k.acme().await;
    let body = mail("<s1@x>", "Hello there", "hi");
    let raw = body.to_string();
    let now = chrono::Utc::now().timestamp();
    let good = |ts: i64| sign(WEBHOOK_SECRET.as_bytes(), ts, raw.as_bytes());

    let cases: Vec<(&str, Vec<(&str, String)>)> = vec![
        ("no headers", vec![]),
        (
            "no signature",
            vec![("x-webhook-timestamp", now.to_string())],
        ),
        ("no timestamp", vec![("x-webhook-signature", good(now))]),
        (
            "bad signature",
            vec![
                ("x-webhook-timestamp", now.to_string()),
                ("x-webhook-signature", "00".repeat(32)),
            ],
        ),
        (
            "signature of another body",
            vec![
                ("x-webhook-timestamp", now.to_string()),
                (
                    "x-webhook-signature",
                    sign(WEBHOOK_SECRET.as_bytes(), now, b"other"),
                ),
            ],
        ),
        (
            "wrong secret",
            vec![
                ("x-webhook-timestamp", now.to_string()),
                (
                    "x-webhook-signature",
                    sign(b"another-secret-another-secret-123", now, raw.as_bytes()),
                ),
            ],
        ),
        (
            "stale",
            vec![
                ("x-webhook-timestamp", (now - 301).to_string()),
                ("x-webhook-signature", good(now - 301)),
            ],
        ),
        (
            "future",
            vec![
                ("x-webhook-timestamp", (now + 301).to_string()),
                ("x-webhook-signature", good(now + 301)),
            ],
        ),
        (
            "timestamp not signed",
            vec![
                ("x-webhook-timestamp", (now - 1000).to_string()),
                ("x-webhook-signature", good(now)),
            ],
        ),
        (
            "non-numeric timestamp",
            vec![
                ("x-webhook-timestamp", "abc".into()),
                ("x-webhook-signature", good(now)),
            ],
        ),
        (
            "odd hex",
            vec![
                ("x-webhook-timestamp", now.to_string()),
                ("x-webhook-signature", "abc".into()),
            ],
        ),
        (
            "non-hex",
            vec![
                ("x-webhook-timestamp", now.to_string()),
                ("x-webhook-signature", "zz".repeat(32)),
            ],
        ),
        (
            "uppercase prefix junk",
            vec![
                ("x-webhook-timestamp", now.to_string()),
                ("x-webhook-signature", format!("sha256={}", good(now))),
            ],
        ),
    ];
    for (name, headers) in cases {
        let h: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let r = k.hook_raw(&raw, &h).await;
        assert_eq!(
            (r.status.as_u16(), r.error_code().as_str()),
            (401, "TOKEN_INVALID"),
            "{name}"
        );
    }
    assert_eq!(k.count("tickets").await, 0);
    // The edges of the window are accepted.
    for ts in [now - 299, now + 299] {
        let r = k
            .hook_at(&mail(&format!("<edge{ts}@x>"), "Edge case", "hi"), ts)
            .await;
        assert_eq!(r.status, 201, "ts {ts}: {}", r.text());
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn webhook_is_off_without_a_secret(pool: PgPool) {
    let k = Kit::build(pool, false, |c| c.mail.webhook_secret = None).await;
    k.acme().await;
    let r = k.hook(mail("<a@x>", "Hello there", "hi")).await;
    assert_eq!(
        (r.status.as_u16(), r.error_code().as_str()),
        (404, "NOT_FOUND")
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn payload_validation(pool: PgPool) {
    let k = Kit::new(pool).await;
    k.acme().await;
    for (name, body) in [
        ("no from", json!({ "to": MAILBOX, "body": "x" })),
        (
            "bad from",
            json!({ "from": "nobody", "to": MAILBOX, "body": "x", "messageId": "<a@x>" }),
        ),
        (
            "bad to",
            json!({ "from": "a@example.com", "to": "nope", "body": "x" }),
        ),
        (
            "no body",
            json!({ "from": "a@example.com", "to": MAILBOX, "subject": "s" }),
        ),
        (
            "bad base64",
            json!({ "from": "a@example.com", "to": MAILBOX, "body": "x", "attachments": [{ "contentBase64": "***" }] }),
        ),
    ] {
        let r = k.hook(body).await;
        assert_eq!(
            (r.status.as_u16(), r.error_code().as_str()),
            (422, "VALIDATION_ERROR"),
            "{name}"
        );
    }
    let raw = "not json";
    let ts = chrono::Utc::now().timestamp();
    let r = k
        .hook_raw(
            raw,
            &[
                ("x-webhook-timestamp", &ts.to_string()),
                (
                    "x-webhook-signature",
                    &sign(WEBHOOK_SECRET.as_bytes(), ts, raw.as_bytes()),
                ),
            ],
        )
        .await;
    assert_eq!(r.status, 422);
    let r = k
        .hook(json!({ "from": "a@example.com", "to": "unknown@acme.test", "body": "x" }))
        .await;
    assert_eq!(
        (r.status.as_u16(), r.error_code().as_str()),
        (404, "NOT_FOUND")
    );
    assert_eq!(k.count("tickets").await, 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn replayed_message_ids_change_nothing(pool: PgPool) {
    let k = Kit::new(pool).await;
    k.acme().await;
    assert_eq!(
        k.hook(mail("<d1@x>", "Hello there", "hi")).await.status,
        201
    );
    let again = k.hook(mail("<d1@x>", "Hello there", "hi")).await;
    assert_eq!(
        (
            again.status.as_u16(),
            again.json()["data"]["duplicate"].clone()
        ),
        (200, json!(true))
    );
    // Same id without brackets is the same mail; a reply replayed is also a no-op.
    assert_eq!(
        k.hook(mail("d1@x", "Hello there", "hi")).await.json()["data"]["duplicate"],
        true
    );
    let reply = json!({ "from": "ann@example.com", "to": MAILBOX, "body": "more", "messageId": "<d2@x>", "inReplyTo": "<d1@x>" });
    assert_eq!(k.hook(reply.clone()).await.status, 201);
    assert_eq!(k.hook(reply).await.json()["data"]["duplicate"], true);
    // No Message-ID: identical content dedupes by digest.
    let anon = json!({ "from": "bob@example.com", "to": MAILBOX, "subject": "No id here", "body": "same" });
    assert_eq!(k.hook(anon.clone()).await.status, 201);
    assert_eq!(k.hook(anon).await.json()["data"]["duplicate"], true);
    assert_eq!(
        (k.count("tickets").await, k.count("messages").await),
        (2, 3)
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn replies_thread_by_number_headers_and_tag(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (_, project) = k.acme().await;
    k.hook(mail("<t1@x>", "Printer", "first")).await;
    let ticket_id: dp_db::Id = sqlx::query_scalar("SELECT id FROM tickets")
        .fetch_one(&k.t.pool)
        .await
        .unwrap();
    let n = |r: &Resp| {
        (
            r.status.as_u16(),
            r.json()["data"]["created"].clone(),
            r.json()["data"]["ticketId"].as_str().map(str::to_string),
        )
    };
    let want = (201, json!(false), Some(ticket_id.to_string()));

    // 1. explicit number from the signed webhook
    let mut b = mail("<r1@x>", "unrelated", "by number");
    b["ticketNumber"] = json!(1);
    assert_eq!(n(&k.hook(b).await), want);
    // 2. In-Reply-To of a stored message, and 3. References
    let mut b = mail("<r2@x>", "whatever", "by header");
    b["inReplyTo"] = json!("<t1@x>");
    assert_eq!(n(&k.hook(b).await), want);
    let mut b = mail("<r3@x>", "whatever", "by references");
    b["references"] = json!("<zzz@x> <r1@x>");
    assert_eq!(n(&k.hook(b).await), want);
    // 4. a Message-ID we minted names the ticket
    let mut b = mail("<r4@x>", "whatever", "by minted id");
    b["inReplyTo"] = json!(format!(
        "<ticket-{ticket_id}-reply-{}@acme.test>",
        uuid::Uuid::new_v4()
    ));
    assert_eq!(n(&k.hook(b).await), want);
    // 5. the subject tag, from the ticket's own client
    assert_eq!(
        n(&k.hook(mail("<r5@x>", "Re: Printer [Ticket #1]", "by tag"))
            .await),
        want
    );
    assert_eq!(k.count("tickets").await, 1);
    assert_eq!(k.count("messages").await, 6);
    let _ = project;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn subject_tag_from_a_stranger_opens_a_new_ticket(pool: PgPool) {
    let k = Kit::new(pool).await;
    k.acme().await;
    k.hook(mail("<t1@x>", "Printer", "first")).await;
    let r = k.hook(json!({ "from": "mallory@evil.test", "to": MAILBOX, "subject": "Re: Printer [Ticket #1]", "body": "inject", "messageId": "<e1@x>" })).await;
    assert_eq!(r.json()["data"]["created"], true);
    let rows = tickets(&k).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].3, "mallory@evil.test");
    let on_first: i64 = sqlx::query_scalar("SELECT count(*) FROM messages m JOIN tickets t ON t.id = m.ticket_id WHERE t.ticket_number = 1").fetch_one(&k.t.pool).await.unwrap();
    assert_eq!(on_first, 1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn threading_never_crosses_tenants(pool: PgPool) {
    let k = Kit::new(pool).await;
    k.acme().await;
    k.hook(mail("<a1@x>", "Printer", "tenant A")).await;
    let owner_b = k.t.user("b@example.com", Role::Agent).await;
    let b = k.t.project(owner_b.id, "beta").await;
    k.configure_mail(b.id, closed_port(), None).await;
    let mailbox_b = "support@beta.test";

    // A's ticket number, A's Message-ID and A's tag, all sent to B's mailbox: B opens its own tickets.
    // (The tag case uses another sender: B's own #1 belongs to ann by then.)
    let cases = [
        json!({ "ticketNumber": 1 }),
        json!({ "inReplyTo": "<a1@x>" }),
        json!({ "subject": "Re: Printer [Ticket #1]", "from": "zed@example.com" }),
    ];
    for (i, extra) in cases.into_iter().enumerate() {
        let mut body = json!({ "from": "ann@example.com", "to": mailbox_b, "subject": "Hello B", "body": "x", "messageId": format!("<b{i}@x>") });
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let r = k.hook(body).await;
        assert_eq!(r.json()["data"]["created"], true, "case {i}");
    }
    let on_a: i64 = sqlx::query_scalar("SELECT count(*) FROM messages m JOIN tickets t ON t.id = m.ticket_id JOIN projects p ON p.id = t.project_id WHERE p.slug = 'acme'").fetch_one(&k.t.pool).await.unwrap();
    assert_eq!(on_a, 1);
    // The same Message-ID in B is a different mail from A's.
    let r = k.hook(json!({ "from": "ann@example.com", "to": mailbox_b, "subject": "Again", "body": "x", "messageId": "<a1@x>" })).await;
    assert_eq!(r.json()["data"]["created"], true);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn customer_reply_reopens_resolved_and_closed_tickets(pool: PgPool) {
    let k = Kit::new(pool).await;
    k.acme().await;
    k.hook(mail("<t1@x>", "Printer", "first")).await;
    for (i, status) in ["resolved", "closed"].into_iter().enumerate() {
        sqlx::query("UPDATE tickets SET status = $1, closed_at = now()")
            .bind(status)
            .execute(&k.t.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM unread_tickets")
            .execute(&k.t.pool)
            .await
            .unwrap();
        let mut b = mail(&format!("<re{i}@x>"), "Re: Printer", "still broken");
        b["inReplyTo"] = json!("<t1@x>");
        assert_eq!(k.hook(b).await.status, 201);
        let (st, closed): (String, Option<chrono::DateTime<chrono::Utc>>) =
            sqlx::query_as("SELECT status, closed_at FROM tickets")
                .fetch_one(&k.t.pool)
                .await
                .unwrap();
        assert_eq!((st.as_str(), closed), ("in_progress", None), "{status}");
        assert_eq!(k.count("unread_tickets").await, 1);
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn loops_and_machine_mail_are_ignored(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (_, project) = k.acme().await;
    let minted = format!("<ticket-{}@acme.test>", uuid::Uuid::new_v4());
    let cases = [
        (
            "auto-submitted",
            json!({ "from": "a@example.com", "autoSubmitted": "auto-replied" }),
        ),
        (
            "precedence bulk",
            json!({ "from": "a@example.com", "precedence": "Bulk" }),
        ),
        (
            "mailer-daemon",
            json!({ "from": "MAILER-DAEMON@example.com" }),
        ),
        (
            "noreply",
            json!({ "from": "No Reply <noreply@example.com>" }),
        ),
        ("own mailbox", json!({ "from": MAILBOX })),
        (
            "own message id",
            json!({ "from": "a@example.com", "messageId": minted }),
        ),
    ];
    for (name, extra) in cases {
        let mut b = json!({ "to": MAILBOX, "subject": "Out of office", "body": "I am away", "messageId": format!("<{name}@x>").replace(' ', "") });
        b.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let r = k.hook(b).await;
        assert_eq!(r.status, 200, "{name}: {}", r.text());
        assert!(r.json()["data"]["ignored"].is_string(), "{name}");
    }
    assert_eq!(
        (k.count("tickets").await, k.count("messages").await),
        (0, 0)
    );
    // "Auto-Submitted: no" and an ordinary precedence are normal mail.
    let ok = k.hook(json!({ "from": "a@example.com", "to": MAILBOX, "subject": "Real mail", "body": "x", "autoSubmitted": "no", "precedence": "first-class" })).await;
    assert_eq!(ok.status, 201);
    let _ = project;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn forwarded_and_html_only_mail(pool: PgPool) {
    let k = Kit::new(pool).await;
    k.acme().await;
    let fwd = "---------- Forwarded message ----------\nFrom: Boss <boss@example.com>\nSubject: Invoice\n\nPlease pay invoice 7.";
    let r = k.hook(mail("<f1@x>", "Fwd: Invoice question", fwd)).await;
    assert_eq!(r.status, 201);
    let html = json!({ "from": "h@example.com", "to": MAILBOX, "subject": "Html only", "bodyHtml": "<p>Hello <b>there</b></p><script>alert(1)</script>", "messageId": "<h1@x>" });
    assert_eq!(k.hook(html).await.status, 201);
    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as("SELECT t.subject, m.body, m.html_body FROM messages m JOIN tickets t ON t.id = m.ticket_id ORDER BY t.ticket_number").fetch_all(&k.t.pool).await.unwrap();
    assert_eq!(rows[0].0, "Invoice question");
    assert!(rows[0].1.contains("Please pay invoice 7."), "{}", rows[0].1);
    assert!(
        rows[1].1.contains("Hello") && !rows[1].1.contains("<b>"),
        "{}",
        rows[1].1
    );
    assert_eq!(rows[1].2, None, "raw HTML is never stored");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn attachments_arrive_sanitised_and_bounded(pool: PgPool) {
    let k = Kit::build(pool, false, |c| {
        c.mail.max_file_bytes = 100;
        c.mail.max_request_bytes = 150;
        c.mail.max_files = 2;
    })
    .await;
    k.acme().await;
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    let mut ok = mail("<at1@x>", "With file", "see file");
    ok["attachments"] = json!([{ "filename": "../../x.sh", "contentType": "text/html; charset=x", "contentBase64": b64(b"echo hi") }]);
    assert_eq!(k.hook(ok).await.status, 201);
    let (name, mime, size): (String, String, i64) =
        sqlx::query_as("SELECT filename, mime_type, size_bytes FROM attachments")
            .fetch_one(&k.t.pool)
            .await
            .unwrap();
    assert_eq!(
        (name.as_str(), mime.as_str(), size),
        ("x.sh", "text/html", 7)
    );
    // Served as a download, never inline.
    let owner_tok = k.t.signin("owner@example.com").await.access_token;
    let id: String = sqlx::query_scalar("SELECT id::text FROM attachments")
        .fetch_one(&k.t.pool)
        .await
        .unwrap();
    let f =
        k.t.send(
            k.t.get(&format!("/attachments/file/{id}"))
                .bearer(&owner_tok),
        )
        .await;
    assert!(
        f.header("content-disposition")
            .unwrap()
            .starts_with("attachment;")
    );
    assert_eq!(f.header("content-type"), Some("application/octet-stream"));

    let big = b64(&[b'a'; 200]);
    for (name, files) in [
        ("too big", json!([{ "contentBase64": big }])),
        (
            "too many",
            json!([{ "contentBase64": b64(b"1") }, { "contentBase64": b64(b"2") }, { "contentBase64": b64(b"3") }]),
        ),
        (
            "total",
            json!([{ "contentBase64": b64(&[b'a'; 90]) }, { "contentBase64": b64(&[b'a'; 90]) }]),
        ),
    ] {
        let mut b = mail(&format!("<{}@x>", name.replace(' ', "")), "Big", "x");
        b["attachments"] = files;
        let r = k.hook(b).await;
        assert_eq!(
            (r.status.as_u16(), r.error_code().as_str()),
            (413, "PAYLOAD_TOO_LARGE"),
            "{name}"
        );
    }
    assert_eq!(
        (k.count("tickets").await, k.count("attachments").await),
        (1, 1)
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn project_quota_drops_attachments_but_keeps_the_mail(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (_, project) = k.acme().await;
    sqlx::query("UPDATE projects SET storage_quota_mb = 0 WHERE id = $1")
        .bind(project.id)
        .execute(&k.t.pool)
        .await
        .unwrap();
    let mut b = mail("<q1@x>", "Quota", "text survives");
    b["attachments"] = json!([{ "filename": "a.txt", "contentBase64": base64::engine::general_purpose::STANDARD.encode(b"abc") }]);
    assert_eq!(k.hook(b).await.status, 201);
    assert_eq!(
        (
            k.count("tickets").await,
            k.count("attachments").await,
            k.stored_files()
        ),
        (1, 0, 0)
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn body_size_cap_and_rate_limit(pool: PgPool) {
    let k = Kit::build(pool, false, |c| c.webhook_limit = 2048).await;
    k.acme().await;
    let r = k.hook(mail("<big@x>", "Too big", &"a".repeat(5000))).await;
    assert_eq!(
        (r.status.as_u16(), r.error_code().as_str()),
        (413, "PAYLOAD_TOO_LARGE")
    );
    // 120 per minute per client, counted before the signature is looked at.
    for i in 0..119 {
        let r = k.hook_raw("{}", &[]).await;
        assert_eq!(r.status, 401, "request {i}");
    }
    let r = k.hook_raw("{}", &[]).await;
    assert_eq!(
        (r.status.as_u16(), r.error_code().as_str()),
        (429, "RATE_LIMITED")
    );
    assert!(r.header("retry-after").is_some());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn identical_mails_with_different_dates_are_not_duplicates(pool: PgPool) {
    let k = Kit::new(pool).await;
    k.acme().await;
    let at = |d: &str| json!({ "from": "bob@example.com", "to": MAILBOX, "subject": "Thanks", "body": "Thanks", "date": d });
    assert_eq!(k.hook(at("2026-11-06T10:00:00Z")).await.status, 201);
    assert_eq!(k.hook(at("2026-11-06T11:00:00Z")).await.status, 201);
    assert_eq!(
        k.hook(at("2026-11-06T11:00:00Z")).await.json()["data"]["duplicate"],
        true
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn unverified_mailbox_does_not_receive_webhook_mail(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (_, project) = k.acme().await;
    // Squatter: a mailbox address that was typed in but never proven by a login.
    sqlx::query("UPDATE projects SET mail_verified_at = NULL WHERE id = $1")
        .bind(project.id)
        .execute(&k.t.pool)
        .await
        .unwrap();
    let r = k.hook(mail("<sq1@x>", "Hello", "for the victim")).await;
    assert_eq!(r.status, 404, "{}", r.text());
    assert!(tickets(&k).await.is_empty());
    // Changing the address of a verified project drops the proof again.
    sqlx::query("UPDATE projects SET mail_verified_at = now() WHERE id = $1")
        .bind(project.id)
        .execute(&k.t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE projects SET smtp_email = 'other@acme.test' WHERE id = $1")
        .bind(project.id)
        .execute(&k.t.pool)
        .await
        .unwrap();
    let r = k.hook(mail("<sq2@x>", "Hello", "again")).await;
    assert_eq!(r.status, 404);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn only_a_global_admin_can_approve_a_mailbox(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (_, project) = k.acme().await;
    sqlx::query("UPDATE projects SET mail_verified_at = NULL WHERE id = $1")
        .bind(project.id)
        .execute(&k.t.pool)
        .await
        .unwrap();
    k.t.user("root@example.com", Role::Admin).await;
    let owner = k.t.signin("owner@example.com").await.access_token;
    let root = k.t.signin("root@example.com").await.access_token;
    let path = format!("/projects/{}/mailbox/approve", project.id);
    let r =
        k.t.send_json(
            k.t.post(&path).bearer(&owner),
            json!({ "smtpEmail": MAILBOX }),
        )
        .await;
    assert_eq!(r.status, 403, "the project owner cannot approve itself");
    assert_eq!(k.hook(mail("<c1@x>", "Hi", "before")).await.status, 404);
    let stale =
        k.t.send_json(
            k.t.post(&path).bearer(&root),
            json!({ "smtpEmail": "other@evil.test" }),
        )
        .await;
    assert_eq!(
        stale.status, 409,
        "approval is bound to the reviewed address"
    );
    let r =
        k.t.send_json(
            k.t.post(&path).bearer(&root),
            json!({ "smtpEmail": MAILBOX }),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(k.hook(mail("<c2@x>", "Hi", "after")).await.status, 201);
}
