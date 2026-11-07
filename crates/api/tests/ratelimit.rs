mod common;

use std::net::IpAddr;
use std::time::{Duration, Instant};

use api::ratelimit::{RateLimiter, Rule};
use axum::http::StatusCode;
use common::*;
use serde_json::json;
use sqlx::PgPool;

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

#[test]
fn allows_the_quota_then_blocks_with_a_wait() {
    let l = RateLimiter::new(100);
    let t0 = Instant::now();
    for i in 0..10 {
        assert!(
            l.check_at(Rule::Login, ip("1.1.1.1"), t0 + Duration::from_secs(i))
                .is_ok()
        );
    }
    let wait = l
        .check_at(Rule::Login, ip("1.1.1.1"), t0 + Duration::from_secs(10))
        .unwrap_err();
    // The oldest hit (t0) leaves the 60 s window at t0 + 60.
    assert_eq!(wait, Duration::from_secs(50));
}

#[test]
fn window_slides_instead_of_resetting() {
    let l = RateLimiter::new(100);
    let t0 = Instant::now();
    for i in 0..10 {
        l.check_at(Rule::Login, ip("1.1.1.1"), t0 + Duration::from_secs(i * 5))
            .unwrap();
    }
    // t0 + 60: only the first hit expired, so exactly one more request fits.
    let t = t0 + Duration::from_secs(60);
    assert!(l.check_at(Rule::Login, ip("1.1.1.1"), t).is_ok());
    assert!(l.check_at(Rule::Login, ip("1.1.1.1"), t).is_err());
    assert!(
        l.check_at(Rule::Login, ip("1.1.1.1"), t + Duration::from_secs(60))
            .is_ok()
    );
}

#[test]
fn rejected_requests_do_not_extend_the_block() {
    let l = RateLimiter::new(100);
    let t0 = Instant::now();
    for _ in 0..10 {
        l.check_at(Rule::Login, ip("1.1.1.1"), t0).unwrap();
    }
    for s in 1..59 {
        assert!(
            l.check_at(Rule::Login, ip("1.1.1.1"), t0 + Duration::from_secs(s))
                .is_err()
        );
    }
    assert!(
        l.check_at(Rule::Login, ip("1.1.1.1"), t0 + Duration::from_secs(60))
            .is_ok()
    );
}

#[test]
fn clients_and_rules_are_independent() {
    let l = RateLimiter::new(100);
    let t = Instant::now();
    for _ in 0..10 {
        l.check_at(Rule::Login, ip("1.1.1.1"), t).unwrap();
    }
    assert!(l.check_at(Rule::Login, ip("1.1.1.1"), t).is_err());
    assert!(l.check_at(Rule::Login, ip("1.1.1.2"), t).is_ok());
    assert!(l.check_at(Rule::Public, ip("1.1.1.1"), t).is_ok());
    assert!(l.check_at(Rule::Webhook, ip("1.1.1.1"), t).is_ok());
}

#[test]
fn rule_quotas() {
    let allowed = |rule| {
        let l = RateLimiter::new(10);
        let t = Instant::now();
        (0..1000)
            .take_while(|_| l.check_at(rule, ip("9.9.9.9"), t).is_ok())
            .count()
    };
    assert_eq!(allowed(Rule::Login), 10);
    assert_eq!(allowed(Rule::Register), 5);
    assert_eq!(allowed(Rule::Refresh), 30);
    assert_eq!(allowed(Rule::Public), 20);
    assert_eq!(allowed(Rule::Webhook), 120);
}

#[test]
fn an_ipv6_64_is_one_client() {
    let l = RateLimiter::new(100);
    let t = Instant::now();
    for i in 0..10u16 {
        l.check_at(Rule::Login, ip(&format!("2001:db8:1:2::{i:x}")), t)
            .unwrap();
    }
    assert!(
        l.check_at(Rule::Login, ip("2001:db8:1:2:ffff::1"), t)
            .is_err()
    );
    assert!(l.check_at(Rule::Login, ip("2001:db8:1:3::1"), t).is_ok());
}

#[test]
fn memory_is_bounded_by_evicting_old_keys() {
    let l = RateLimiter::new(50);
    let t0 = Instant::now();
    for i in 0..500u32 {
        let a = IpAddr::from(i.to_be_bytes());
        l.check_at(Rule::Login, a, t0 + Duration::from_millis(i as u64))
            .unwrap();
        assert!(l.tracked_keys() <= 50);
    }
    assert_eq!(l.tracked_keys(), 50);
}

#[test]
fn expired_keys_go_before_live_ones() {
    let l = RateLimiter::new(2);
    let t0 = Instant::now();
    l.check_at(Rule::Login, ip("1.0.0.1"), t0).unwrap();
    l.check_at(Rule::Login, ip("1.0.0.2"), t0 + Duration::from_secs(100))
        .unwrap();
    // Full; the first key is long expired, so the live second key survives the newcomer.
    l.check_at(Rule::Login, ip("1.0.0.3"), t0 + Duration::from_secs(101))
        .unwrap();
    let live: Vec<_> = (0..10)
        .take_while(|_| {
            l.check_at(Rule::Login, ip("1.0.0.2"), t0 + Duration::from_secs(101))
                .is_ok()
        })
        .collect();
    assert_eq!(live.len(), 9, "second key kept its earlier hit");
}

async fn signin_attempt(
    t: &TestApp,
    peer_headers: Option<(&str, &str)>,
    peer: &str,
) -> axum::http::StatusCode {
    let mut req = t.post("/auth/signin").peer(peer);
    if let Some((k, v)) = peer_headers {
        req = req.header(k, v);
    }
    t.send_json(
        req,
        json!({ "email": "nobody@example.com", "password": "whatever-1" }),
    )
    .await
    .status
}

#[sqlx::test(migrations = "../db/migrations")]
async fn signin_is_limited_with_retry_after(pool: PgPool) {
    let t = TestApp::new(pool).await;
    for _ in 0..10 {
        assert_eq!(
            signin_attempt(&t, None, "198.51.100.1").await,
            StatusCode::UNAUTHORIZED
        );
    }
    let res = t
        .send_json(
            t.post("/auth/signin").peer("198.51.100.1"),
            json!({ "email": "nobody@example.com", "password": "whatever-1" }),
        )
        .await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(res.error_code(), "RATE_LIMITED");
    let wait: u64 = res.header("retry-after").unwrap().parse().unwrap();
    assert!((1..=60).contains(&wait));
    // Another client is unaffected.
    assert_eq!(
        signin_attempt(&t, None, "198.51.100.2").await,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn signup_and_refresh_have_their_own_limits(pool: PgPool) {
    let t = TestApp::new(pool).await;
    for i in 0..5 {
        let res = t
            .send_json(
                t.post("/auth/signup").peer("198.51.100.7"),
                json!({ "email": format!("u{i}@example.com"), "password": PASSWORD, "name": "U" }),
            )
            .await;
        assert_eq!(res.status, StatusCode::CREATED);
    }
    let res = t
        .send_json(
            t.post("/auth/signup").peer("198.51.100.7"),
            json!({ "email": "u9@example.com", "password": PASSWORD, "name": "U" }),
        )
        .await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS);
    // Signup being exhausted does not block sign-in or refresh for that client.
    assert_eq!(
        signin_attempt(&t, None, "198.51.100.7").await,
        StatusCode::UNAUTHORIZED
    );
    for _ in 0..30 {
        let res = t.send(t.post("/auth/refresh").peer("198.51.100.7")).await;
        assert_eq!(res.error_code(), "TOKEN_MISSING");
    }
    let res = t.send(t.post("/auth/refresh").peer("198.51.100.7")).await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn forwarded_for_from_a_stranger_does_not_dodge_the_limit(pool: PgPool) {
    let t = TestApp::new(pool).await;
    for i in 0..10 {
        let spoof = format!("203.0.113.{i}");
        assert_eq!(
            signin_attempt(&t, Some(("x-forwarded-for", &spoof)), "198.51.100.1").await,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        signin_attempt(
            &t,
            Some(("x-forwarded-for", "203.0.113.200")),
            "198.51.100.1"
        )
        .await,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn trusted_proxy_separates_clients_by_forwarded_address(pool: PgPool) {
    let t = TestApp::new(pool).await;
    // The proxy (10.0.0.1) appends the real client; a spoofed leftmost entry is ignored.
    for _ in 0..10 {
        let s = signin_attempt(
            &t,
            Some(("x-forwarded-for", "7.7.7.7, 203.0.113.50")),
            "10.0.0.1",
        )
        .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
    let blocked = signin_attempt(
        &t,
        Some(("x-forwarded-for", "8.8.8.8, 203.0.113.50")),
        "10.0.0.1",
    )
    .await;
    assert_eq!(blocked, StatusCode::TOO_MANY_REQUESTS);
    let other = signin_attempt(&t, Some(("x-forwarded-for", "203.0.113.51")), "10.0.0.1").await;
    assert_eq!(other, StatusCode::UNAUTHORIZED);
}

#[test]
fn scale_multiplies_the_quota() {
    let l = RateLimiter::new(100).with_scale(3);
    let t0 = Instant::now();
    for _ in 0..30 {
        assert!(l.check_at(Rule::Login, ip("1.1.1.1"), t0).is_ok());
    }
    assert!(l.check_at(Rule::Login, ip("1.1.1.1"), t0).is_err());
}
