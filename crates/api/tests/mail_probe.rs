mod common;
mod r21_world;

use axum::http::Method;
use r21_world::*;
use sd_mail::testing::{FakeImap, FakeSmtp};
use serde_json::json;
use sqlx::PgPool;

fn smtp_path(w: &W) -> String {
    format!("/projects/{}/test-smtp", w.project.id)
}

fn imap_path(w: &W) -> String {
    format!("/projects/{}/test-imap", w.project.id)
}

async fn local_world(pool: PgPool) -> W {
    world_with(TestApp::with_config(pool, config_allowing_private_hosts()).await).await
}

#[sqlx::test(migrations = "../db/migrations")]
async fn probes_refuse_loopback_and_private_targets(pool: PgPool) {
    let w = world(pool).await;
    // Each probe call counts against the limiter, so use a fresh app per path batch via peers.
    let hosts = [
        "127.0.0.1",
        "localhost",
        "10.0.0.1",
        "192.168.1.1",
        "169.254.169.254",
        "::1",
        "0.0.0.0",
        "2130706433",
        "db.internal",
        "intranet",
    ];
    for (n, host) in hosts.iter().enumerate() {
        let path = if n % 2 == 0 {
            smtp_path(&w)
        } else {
            imap_path(&w)
        };
        let ip = format!("198.51.100.{}", n + 1);
        let req =
            w.t.post(&path.replacen("/api/v3", "", 1))
                .bearer(&w.owner.token)
                .peer(&ip);
        let res =
            w.t.send_json(
                req,
                json!({ "host": host, "port": 25, "username": "u", "password": "p" }),
            )
            .await;
        assert_eq!(res.status, 422, "{path} {host}: {}", res.text());
        assert!(
            res.json()["error"]["details"].get("host").is_some(),
            "{host}"
        );
    }
    // Plaintext transport is only for local test servers.
    let res = w
        .post(
            &smtp_path(&w),
            &w.owner.token,
            json!({ "host": "mail.example.com", "tls": "none", "password": "p" }),
        )
        .await;
    assert_eq!(res.status, 422);
    assert!(res.json()["error"]["details"].get("tls").is_some());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn probe_role_matrix_and_validation(pool: PgPool) {
    let w = world(pool).await;
    let body = Some(json!({ "host": "127.0.0.1", "password": "p" }));
    assert_eq!(
        w.matrix(Method::POST, &smtp_path(&w), body).await,
        [422, 422, 403, 404, 404]
    );
    assert_eq!(
        w.call(Method::POST, &smtp_path(&w), None, Some(json!({})))
            .await
            .status,
        401
    );
    // No host stored and none given.
    let req =
        w.t.post(&format!("/projects/{}/test-imap", w.project.id))
            .bearer(&w.owner.token)
            .peer("198.51.100.200");
    let r = w.t.send_json(req, json!({})).await;
    assert_eq!(r.status, 422);
    assert!(r.json()["error"]["details"].get("host").is_some());
    let bad_bodies = [
        json!({ "host": "a b" }),
        json!({ "host": "mail.example.com", "port": 0 }),
        json!({ "host": "mail.example.com", "tls": "ssl" }),
    ];
    for (i, bad) in bad_bodies.into_iter().enumerate() {
        let req =
            w.t.post(&format!("/projects/{}/test-smtp", w.project.id))
                .bearer(&w.admin.token)
                .peer(&format!("198.51.100.{}", 100 + i));
        assert_eq!(w.t.send_json(req, bad).await.status, 422);
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn probes_are_rate_limited(pool: PgPool) {
    let w = world(pool).await;
    let mut codes = Vec::new();
    for _ in 0..7 {
        codes.push(
            w.post(
                &smtp_path(&w),
                &w.owner.token,
                json!({ "host": "127.0.0.1" }),
            )
            .await
            .status
            .as_u16(),
        );
    }
    assert_eq!(&codes[..5], &[422; 5]);
    assert_eq!(&codes[5..], &[429, 429]);
    // IMAP shares the limiter, so it is throttled too.
    let r = w
        .post(
            &imap_path(&w),
            &w.owner.token,
            json!({ "host": "127.0.0.1" }),
        )
        .await;
    assert_eq!(r.status, 429);
    assert!(r.headers.contains_key("retry-after"));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn smtp_probe_reports_success_and_failure_without_leaking(pool: PgPool) {
    let w = local_world(pool).await;
    let smtp = FakeSmtp::start_requiring_auth("mailer", "right-pw").await;
    let target = |pw: &str| json!({ "host": "127.0.0.1", "port": smtp.port(), "tls": "none", "username": "mailer", "password": pw });
    let ok = w
        .post(&smtp_path(&w), &w.owner.token, target("right-pw"))
        .await;
    assert_eq!(ok.status, 200, "{}", ok.text());
    assert_eq!(data(&ok)["ok"], true);
    let bad = w
        .post(&smtp_path(&w), &w.owner.token, target("wrong-pw-xyz"))
        .await;
    assert_eq!(bad.status, 200);
    assert_eq!(data(&bad)["ok"], false);
    assert_no_secrets(&bad, &["wrong-pw-xyz", "right-pw"]);
    // A closed port is a clean failure, not a 500.
    let closed = w.post(&smtp_path(&w), &w.owner.token, json!({ "host": "127.0.0.1", "port": 1, "tls": "none", "username": "u", "password": "p" })).await;
    assert_eq!(
        (closed.status.as_u16(), data(&closed)["ok"].clone()),
        (200, json!(false))
    );
    assert!(
        !closed.text().contains("127.0.0.1"),
        "no address or OS error in the message"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn imap_probe_uses_the_stored_password_only_for_the_stored_target(pool: PgPool) {
    let w = local_world(pool).await;
    let imap = FakeImap::start("reader", "stored-pw").await;
    let path = format!("/projects/{}", w.project.id);
    let save = w
        .patch(&path, &w.owner.token, json!({ "imapHost": "127.0.0.1", "imapPort": imap.port(), "imapUsername": "reader", "imapPassword": "stored-pw" }))
        .await;
    assert_eq!(save.status, 200, "{}", save.text());

    // Same target, no password in the request: the stored one is decrypted and used.
    let ok = w
        .post(&imap_path(&w), &w.admin.token, json!({ "tls": "none" }))
        .await;
    assert_eq!(
        (ok.status.as_u16(), data(&ok)["ok"].clone()),
        (200, json!(true)),
        "{}",
        ok.text()
    );
    assert_no_secrets(&ok, &["stored-pw"]);
    // An explicit wrong password wins over the stored one.
    let bad = w
        .post(
            &imap_path(&w),
            &w.admin.token,
            json!({ "tls": "none", "password": "nope" }),
        )
        .await;
    assert_eq!(data(&bad)["ok"], false);
    // A different user, host or port never borrows the stored password.
    for other in [
        json!({ "tls": "none", "username": "someone" }),
        json!({ "tls": "none", "host": "127.0.0.2" }),
        json!({ "tls": "none", "port": imap.port() as i64 + 1 }),
    ] {
        let r = w.post(&imap_path(&w), &w.admin.token, other.clone()).await;
        assert_eq!(r.status, 422, "{other}");
        assert!(r.json()["error"]["details"].get("password").is_some());
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn stored_credentials_are_not_probeable_by_agents_or_outsiders(pool: PgPool) {
    let w = local_world(pool).await;
    assert_eq!(
        w.post(&imap_path(&w), &w.agent.token, json!({ "tls": "none" }))
            .await
            .status,
        403
    );
    assert_eq!(
        w.post(&imap_path(&w), &w.other.token, json!({ "tls": "none" }))
            .await
            .status,
        404
    );
}
