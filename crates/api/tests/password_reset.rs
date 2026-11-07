mod common;

use std::time::Duration;

use api::config::Config;
use axum::http::StatusCode;
use chrono::Utc;
use common::*;
use sd_core::api_key::hash_api_key;
use sd_db::{Role, resets};
use sd_mail::testing::FakeSmtp;
use serde_json::json;
use sqlx::PgPool;

const NEW_PASSWORD: &str = "brand-new-pass-9";

async fn app_with_mail(pool: PgPool) -> (TestApp, FakeSmtp) {
    let smtp = FakeSmtp::start().await;
    let port = smtp.port().to_string();
    let mut c = Config::from_lookup(env_with(&[
        ("SD_MAIL_PLAINTEXT", "1"),
        ("SD_SYSTEM_SMTP_HOST", "127.0.0.1"),
        ("SD_SYSTEM_SMTP_PORT", &port),
        ("SD_SYSTEM_SMTP_FROM", "noreply@desk.test"),
    ]))
    .expect("config");
    let base = test_config();
    c.argon2 = base.argon2;
    (TestApp::with_config(pool, c).await, smtp)
}

async fn forgot(t: &TestApp, email: &str) -> Resp {
    t.send_json(t.post("/auth/forgot-password"), json!({ "email": email }))
        .await
}

async fn reset(t: &TestApp, token: &str, password: &str) -> Resp {
    t.send_json(
        t.post("/auth/reset-password"),
        json!({ "token": token, "password": password }),
    )
    .await
}

/// Waits for `n` mails (the send runs in the background).
async fn mails(smtp: &FakeSmtp, n: usize) -> Vec<sd_mail::testing::ReceivedMail> {
    for _ in 0..100 {
        if smtp.messages().len() >= n {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    smtp.messages()
}

/// Pulls the reset token out of the mail body (quoted-printable decoded).
fn token_in(mail: &sd_mail::testing::ReceivedMail) -> String {
    let raw = String::from_utf8_lossy(&mail.data).replace("=\r\n", "");
    let raw = raw.replace("=3D", "=");
    let start = raw.find("/reset-password/#token=").expect("link in mail")
        + "/reset-password/#token=".len();
    raw[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn disabled_without_a_system_mailbox(pool: PgPool) {
    let t = TestApp::new(pool).await;
    t.user("a@x.test", Role::Agent).await;
    assert_eq!(forgot(&t, "a@x.test").await.status, StatusCode::NOT_FOUND);
    assert_eq!(
        reset(&t, "whatever", NEW_PASSWORD).await.status,
        StatusCode::NOT_FOUND
    );
    let cfg = t.send(t.get("/auth/config")).await.json();
    assert_eq!(cfg["data"]["passwordResetEnabled"], false);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn full_flow_changes_the_password(pool: PgPool) {
    let (t, smtp) = app_with_mail(pool).await;
    t.user("a@x.test", Role::Agent).await;
    let cfg = t.send(t.get("/auth/config")).await.json();
    assert_eq!(cfg["data"]["passwordResetEnabled"], true);

    assert_eq!(forgot(&t, "A@X.test ").await.status, StatusCode::OK);
    let sent = mails(&smtp, 1).await;
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].rcpt_to, vec!["a@x.test".to_string()]);
    let text = String::from_utf8_lossy(&sent[0].data).into_owned();
    assert!(text.contains("noreply@desk.test"), "{text}");
    let token = token_in(&sent[0]);
    assert!(token.len() >= 40, "{token}");
    // Link goes to the configured front-end.
    assert!(
        text.replace("=\r\n", "")
            .contains(&format!("{FRONTEND}/reset-password/"))
    );

    let res = reset(&t, &token, NEW_PASSWORD).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.text());

    let old = t
        .send_json(
            t.post("/auth/signin"),
            json!({ "email": "a@x.test", "password": PASSWORD }),
        )
        .await;
    assert_eq!(old.status, StatusCode::UNAUTHORIZED);
    let new = t
        .send_json(
            t.post("/auth/signin"),
            json!({ "email": "a@x.test", "password": NEW_PASSWORD }),
        )
        .await;
    assert_eq!(new.status, StatusCode::OK);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn known_unknown_and_malformed_addresses_look_the_same(pool: PgPool) {
    let (t, smtp) = app_with_mail(pool).await;
    t.user("a@x.test", Role::Agent).await;
    sqlx::query("INSERT INTO users (id, email, name, password_hash, role, status) VALUES (gen_random_uuid(), 'off@x.test', 'Off', 'h', 'agent', 'inactive')")
        .execute(&t.pool)
        .await
        .unwrap();
    let mut seen = Vec::new();
    for email in [
        "a@x.test",
        "nobody@x.test",
        "off@x.test",
        "not-an-email",
        "",
    ] {
        let res = forgot(&t, email).await;
        seen.push((res.status, res.text()));
    }
    assert!(seen.iter().all(|s| s == &seen[0]), "{seen:?}");
    assert_eq!(seen[0].0, StatusCode::OK);
    // Only the active, existing account got mail.
    let sent = mails(&smtp, 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let sent2 = smtp.messages();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent2.len(), 1, "no mail for unknown/inactive/malformed");
    assert_eq!(sent2[0].rcpt_to, vec!["a@x.test".to_string()]);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn token_works_once(pool: PgPool) {
    let (t, smtp) = app_with_mail(pool).await;
    t.user("a@x.test", Role::Agent).await;
    forgot(&t, "a@x.test").await;
    let token = token_in(&mails(&smtp, 1).await[0]);
    assert_eq!(reset(&t, &token, NEW_PASSWORD).await.status, StatusCode::OK);
    let replay = reset(&t, &token, "attacker-pass-77").await;
    assert_eq!(replay.status, StatusCode::BAD_REQUEST);
    assert_eq!(replay.error_code(), "BAD_REQUEST");
    // The replay changed nothing.
    let ok = t
        .send_json(
            t.post("/auth/signin"),
            json!({ "email": "a@x.test", "password": NEW_PASSWORD }),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn expired_unknown_and_empty_tokens_get_the_same_answer(pool: PgPool) {
    let (t, _smtp) = app_with_mail(pool).await;
    let u = t.user("a@x.test", Role::Agent).await;
    resets::create(
        &t.pool,
        u.id,
        &hash_api_key("expired-token"),
        Utc::now() - chrono::Duration::seconds(1),
    )
    .await
    .unwrap();
    let mut bodies = Vec::new();
    for tok in ["expired-token", "never-issued", "", &"x".repeat(500)] {
        let res = reset(&t, tok, NEW_PASSWORD).await;
        assert_eq!(res.status, StatusCode::BAD_REQUEST, "{tok}");
        bodies.push(res.text());
    }
    assert!(bodies.iter().all(|b| b == &bodies[0]));
    // Password unchanged.
    let old = t
        .send_json(
            t.post("/auth/signin"),
            json!({ "email": "a@x.test", "password": PASSWORD }),
        )
        .await;
    assert_eq!(old.status, StatusCode::OK);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn weak_password_is_rejected_and_the_token_survives(pool: PgPool) {
    let (t, smtp) = app_with_mail(pool).await;
    t.user("a@x.test", Role::Agent).await;
    forgot(&t, "a@x.test").await;
    let token = token_in(&mails(&smtp, 1).await[0]);
    let weak = reset(&t, &token, "short").await;
    assert_eq!(weak.status, StatusCode::UNPROCESSABLE_ENTITY);
    let long = reset(&t, &token, &"p".repeat(129)).await;
    assert_eq!(long.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(reset(&t, &token, NEW_PASSWORD).await.status, StatusCode::OK);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reset_revokes_every_session(pool: PgPool) {
    let (t, smtp) = app_with_mail(pool).await;
    t.user("a@x.test", Role::Agent).await;
    let s1 = t.signin("a@x.test").await;
    let s2 = t.signin("a@x.test").await;
    forgot(&t, "a@x.test").await;
    let token = token_in(&mails(&smtp, 1).await[0]);
    assert_eq!(reset(&t, &token, NEW_PASSWORD).await.status, StatusCode::OK);
    for s in [s1, s2] {
        let res = t
            .send(t.post("/auth/refresh").cookie(&s.cookie_pair()))
            .await;
        assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn a_newer_link_replaces_the_older_one(pool: PgPool) {
    let (t, smtp) = app_with_mail(pool).await;
    t.user("a@x.test", Role::Agent).await;
    forgot(&t, "a@x.test").await;
    let first = token_in(&mails(&smtp, 1).await[0]);
    forgot(&t, "a@x.test").await;
    let all = mails(&smtp, 2).await;
    let second = token_in(&all[1]);
    assert_ne!(first, second);
    assert_eq!(
        reset(&t, &first, NEW_PASSWORD).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        reset(&t, &second, NEW_PASSWORD).await.status,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reset_mails_are_capped_per_address_without_saying_so(pool: PgPool) {
    let (t, smtp) = app_with_mail(pool).await;
    t.user("a@x.test", Role::Agent).await;
    for i in 0..5 {
        let res = forgot(&t, "a@x.test").await;
        assert_eq!(res.status, StatusCode::OK, "request {i}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    mails(&smtp, 3).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(smtp.messages().len(), 3);
    // An unknown address is counted the same way (the cap cannot tell accounts apart).
    for _ in 0..5 {
        assert_eq!(forgot(&t, "ghost@x.test").await.status, StatusCode::OK);
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn requests_are_capped_per_ip(pool: PgPool) {
    let (t, _smtp) = app_with_mail(pool).await;
    for i in 0..10 {
        let res = forgot(&t, &format!("u{i}@x.test")).await;
        assert_eq!(res.status, StatusCode::OK, "request {i}");
    }
    let res = forgot(&t, "u99@x.test").await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(res.header("retry-after").is_some());
    // A different client is unaffected.
    let other = t
        .send_json(
            t.post("/auth/forgot-password").peer("198.51.100.7"),
            json!({ "email": "u1@x.test" }),
        )
        .await;
    assert_eq!(other.status, StatusCode::OK);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn token_guessing_is_throttled(pool: PgPool) {
    let (t, _smtp) = app_with_mail(pool).await;
    for _ in 0..10 {
        assert_eq!(
            reset(&t, "guess", NEW_PASSWORD).await.status,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        reset(&t, "guess", NEW_PASSWORD).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn a_failed_mail_still_answers_200(pool: PgPool) {
    // Nothing listens on this port: the background send fails and is only logged.
    let c = {
        let mut c = Config::from_lookup(env_with(&[
            ("SD_MAIL_PLAINTEXT", "1"),
            ("SD_SYSTEM_SMTP_HOST", "127.0.0.1"),
            ("SD_SYSTEM_SMTP_PORT", "1"),
            ("SD_SYSTEM_SMTP_FROM", "noreply@desk.test"),
        ]))
        .unwrap();
        c.argon2 = test_config().argon2;
        c
    };
    let t = TestApp::with_config(pool, c).await;
    t.user("a@x.test", Role::Agent).await;
    let res = forgot(&t, "a@x.test").await;
    assert_eq!(res.status, StatusCode::OK);
}
