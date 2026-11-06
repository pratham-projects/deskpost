mod common;

use api::config::Argon2Cost;
use api::password::Hasher;
use api::routes::auth::create_admin;
use axum::http::StatusCode;
use common::*;
use sd_db::{Role, sessions, users};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

async fn signup(t: &TestApp, email: &str) -> Resp {
    t.send_json(
        t.post("/auth/signup"),
        json!({ "email": email, "password": PASSWORD, "name": "Ada" }),
    )
    .await
}

async fn refresh(t: &TestApp, cookie: &str) -> Resp {
    t.send(t.post("/auth/refresh").cookie(cookie)).await
}

async fn family_of(t: &TestApp, email: &str) -> sd_db::Id {
    sqlx::query_scalar("SELECT f.id FROM refresh_families f JOIN users u ON u.id = f.user_id WHERE u.email = $1 ORDER BY f.created_at DESC LIMIT 1")
        .bind(email)
        .fetch_one(&t.pool)
        .await
        .unwrap()
}

// ---- signup -------------------------------------------------------------------------------

#[sqlx::test(migrations = "../db/migrations")]
async fn signup_creates_first_admin_then_agents(pool: PgPool) {
    let t = TestApp::new(pool).await;
    let res = signup(&t, "  Ada@Example.com ").await;
    assert_eq!(res.status, StatusCode::CREATED);
    let j = res.json();
    assert_eq!(j["success"], true);
    assert_eq!(j["data"]["email"], "ada@example.com");
    assert_eq!(j["data"]["role"], "admin");
    assert!(j["data"]["id"].as_str().unwrap().parse::<Uuid>().is_ok());
    assert!(j["data"].get("passwordHash").is_none() && !res.text().contains("argon2"));
    assert_eq!(
        signup(&t, "bob@example.com").await.json()["data"]["role"],
        "agent"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn passwords_are_stored_as_argon2id(pool: PgPool) {
    let t = TestApp::new(pool).await;
    signup(&t, "ada@example.com").await;
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM users")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert!(hash.starts_with("$argon2id$"), "{hash}");
    assert!(!hash.contains(PASSWORD));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn signup_email_is_unique_case_insensitively(pool: PgPool) {
    let t = TestApp::new(pool).await;
    assert_eq!(
        signup(&t, "ada@example.com").await.status,
        StatusCode::CREATED
    );
    let res = signup(&t, "ADA@EXAMPLE.COM").await;
    assert_eq!(
        (res.status, res.error_code().as_str()),
        (StatusCode::CONFLICT, "CONFLICT")
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn signup_validation_reports_every_field(pool: PgPool) {
    let t = TestApp::new(pool).await;
    let res = t
        .send_json(
            t.post("/auth/signup"),
            json!({ "email": "nope", "password": "short", "name": "  " }),
        )
        .await;
    assert_eq!(
        (res.status, res.error_code().as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR")
    );
    let d = &res.json()["error"]["details"];
    assert!(
        d["email"].is_string() && d["password"].is_string() && d["name"].is_string(),
        "{d}"
    );

    let too_long = "x".repeat(129);
    let res = t
        .send_json(
            t.post("/auth/signup"),
            json!({ "email": "a@example.com", "password": too_long, "name": "A" }),
        )
        .await;
    assert!(res.json()["error"]["details"]["password"].is_string());
    let res = t
        .send_json(
            t.post("/auth/signup"),
            json!({ "email": "a@example.com", "password": PASSWORD, "name": "n".repeat(256) }),
        )
        .await;
    assert!(res.json()["error"]["details"]["name"].is_string());

    // Missing fields are validation errors, not 400/500.
    let res = t.send_json(t.post("/auth/signup"), json!({})).await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        users::get_by_email(&t.pool, "a@example.com")
            .await
            .unwrap()
            .is_none()
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn bodies_that_are_not_json_objects_get_envelope_errors(pool: PgPool) {
    let t = TestApp::new(pool).await;
    let post = |body: &'static str, ct: &'static str| {
        let t = &t;
        async move {
            t.send_body(
                t.post("/auth/signin").header("content-type", ct),
                body.into(),
            )
            .await
        }
    };
    let res = post("{not json", "application/json").await;
    assert_eq!(
        (res.status, res.error_code().as_str()),
        (StatusCode::BAD_REQUEST, "BAD_REQUEST")
    );
    let res = post("{}", "text/plain").await;
    assert_eq!(
        (res.status, res.error_code().as_str()),
        (StatusCode::UNSUPPORTED_MEDIA_TYPE, "UNSUPPORTED_MEDIA_TYPE")
    );
    let res = post("[1,2]", "application/json").await;
    assert_eq!(
        (res.status, res.error_code().as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR")
    );
    let res = post(r#"{"email": 5, "password": true}"#, "application/json").await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);
    let res = t.send(t.post("/auth/signin")).await;
    assert_eq!(res.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn signup_can_be_disabled(pool: PgPool) {
    let mut c = test_config();
    c.allow_signup = false;
    let t = TestApp::with_config(pool, c).await;
    let res = signup(&t, "ada@example.com").await;
    assert_eq!(
        (res.status, res.error_code().as_str()),
        (StatusCode::FORBIDDEN, "FORBIDDEN")
    );
    assert_eq!(common_count(&t).await, 0);
    let res = t.send(t.get("/auth/config")).await;
    assert_eq!(res.json()["data"]["signupEnabled"], false);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn signup_defaults_are_closed_and_never_mint_an_admin(pool: PgPool) {
    // Production defaults: sign-up off.
    let mut c = test_config();
    c.allow_signup = false;
    c.first_user_admin = false;
    let t = TestApp::with_config(pool.clone(), c).await;
    assert_eq!(
        signup(&t, "ada@example.com").await.status,
        StatusCode::FORBIDDEN
    );
    // Sign-up opened but no explicit first-admin opt-in: first user is an agent.
    let mut c = test_config();
    c.first_user_admin = false;
    let t = TestApp::with_config(pool, c).await;
    let res = signup(&t, "ada@example.com").await;
    assert_eq!(res.json()["data"]["role"], "agent");
}

async fn common_count(t: &TestApp) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&t.pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn auth_config_is_public(pool: PgPool) {
    let t = TestApp::new(pool).await;
    let res = t.send(t.get("/auth/config")).await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.json()["data"]["signupEnabled"], true);
}

// ---- signin -------------------------------------------------------------------------------

#[sqlx::test(migrations = "../db/migrations")]
async fn signin_returns_tokens_and_a_dev_cookie(pool: PgPool) {
    let t = TestApp::new(pool).await;
    t.user("ada@example.com", Role::Agent).await;
    let res = t
        .send_json(
            t.post("/auth/signin"),
            json!({ "email": "  ADA@example.com", "password": PASSWORD }),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK);
    let d = &res.json()["data"];
    assert_eq!(d["user"]["email"], "ada@example.com");
    assert_eq!(d["user"]["role"], "agent");
    assert_eq!(d["user"]["status"], "active");
    assert!(d["user"].get("passwordHash").is_none() && !res.text().contains("$argon2"));
    assert_eq!(d["expiresIn"], 300);
    let access = d["accessToken"].as_str().unwrap();
    assert_eq!(access.split('.').count(), 3);
    let refresh = d["refreshToken"]
        .as_str()
        .expect("dev mode returns the refresh token");
    assert!(refresh.len() >= 43);

    let cookie = res.set_cookie().unwrap();
    assert!(
        cookie.starts_with(&format!("refresh_token={refresh};")),
        "{cookie}"
    );
    for flag in [
        "HttpOnly",
        "Path=/api/v3/auth",
        "SameSite=Lax",
        "Max-Age=604800",
    ] {
        assert!(cookie.contains(flag), "{flag} in {cookie}");
    }
    assert!(!cookie.contains("Secure"));

    let me = t.send(t.get("/auth/me").bearer(access)).await;
    assert_eq!(me.json()["data"]["user"]["email"], "ada@example.com");
    assert!(me.json()["data"]["user"]["lastLoginAt"].is_string());
    // Only the hash of the refresh token is stored.
    let stored: Vec<String> = sqlx::query_scalar("SELECT token_hash FROM refresh_tokens")
        .fetch_all(&t.pool)
        .await
        .unwrap();
    assert_eq!(stored.len(), 1);
    assert!(!stored[0].contains(refresh) && stored[0].len() == 64);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn production_cookie_is_strict_secure_and_token_stays_out_of_the_body(pool: PgPool) {
    let c = api::config::Config::from_lookup(env_with(&[("SD_ENV", "production")])).unwrap();
    let mut c = c;
    c.argon2 = Argon2Cost {
        memory_kib: 8,
        iterations: 1,
        parallelism: 1,
    };
    let t = TestApp::with_config(pool, c).await;
    t.user("ada@example.com", Role::Agent).await;
    let res = t
        .send_json(
            t.post("/auth/signin"),
            json!({ "email": "ada@example.com", "password": PASSWORD }),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK);
    assert!(res.json()["data"].get("refreshToken").is_none());
    let cookie = res.set_cookie().unwrap();
    for flag in ["HttpOnly", "Secure", "SameSite=Strict", "Path=/api/v3/auth"] {
        assert!(cookie.contains(flag), "{flag} in {cookie}");
    }
    assert!(res.header("strict-transport-security").is_some());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn api_version_changes_route_and_cookie_path(pool: PgPool) {
    let c = api::config::Config::from_lookup(env_with(&[("API_VERSION", "v9")])).unwrap();
    let mut c = c;
    c.argon2 = Argon2Cost {
        memory_kib: 8,
        iterations: 1,
        parallelism: 1,
    };
    let t = TestApp::with_config(pool, c).await;
    t.user("ada@example.com", Role::Agent).await;
    let s = t.signin("ada@example.com").await;
    assert!(s.cookie.contains("Path=/api/v9/auth"));
    let old = t.send(t.get("/auth/me").bearer(&s.access_token)).await;
    assert_eq!(old.status, StatusCode::OK);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn signin_failures_are_indistinguishable(pool: PgPool) {
    let t = TestApp::new(pool).await;
    t.user("ada@example.com", Role::Agent).await;
    t.user("idle@example.com", Role::Agent).await;
    t.user("banned@example.com", Role::Agent).await;
    sqlx::query("UPDATE users SET status = 'inactive' WHERE email = 'idle@example.com'")
        .execute(&t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET status = 'suspended' WHERE email = 'banned@example.com'")
        .execute(&t.pool)
        .await
        .unwrap();

    let mut seen = Vec::new();
    for (email, password) in [
        ("ghost@example.com", PASSWORD),
        ("ada@example.com", "wrong-password-1"),
        ("idle@example.com", PASSWORD),
        ("banned@example.com", PASSWORD),
        ("ADA@EXAMPLE.COM", "wrong-password-1"),
    ] {
        let res = t
            .send_json(
                t.post("/auth/signin"),
                json!({ "email": email, "password": password }),
            )
            .await;
        assert_eq!(res.status, StatusCode::UNAUTHORIZED, "{email}");
        assert!(res.set_cookie().is_none(), "{email}");
        seen.push(res.text());
    }
    assert!(
        seen.windows(2).all(|w| w[0] == w[1]),
        "responses differ: {seen:?}"
    );
    assert!(seen[0].contains("INVALID_CREDENTIALS"));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn signin_validation(pool: PgPool) {
    let t = TestApp::new(pool).await;
    for body in [
        json!({}),
        json!({ "email": "", "password": "" }),
        json!({ "email": "a@b.co", "password": "p".repeat(129) }),
    ] {
        let res = t.send_json(t.post("/auth/signin"), body).await;
        assert_eq!(
            (res.status, res.error_code().as_str()),
            (StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR")
        );
    }
}

// ---- refresh ------------------------------------------------------------------------------

#[sqlx::test(migrations = "../db/migrations")]
async fn refresh_rotates_the_token(pool: PgPool) {
    let t = TestApp::new(pool).await;
    t.user("ada@example.com", Role::Agent).await;
    let s = t.signin("ada@example.com").await;
    let res = refresh(&t, &s.cookie_pair()).await;
    assert_eq!(res.status, StatusCode::OK);
    let d = &res.json()["data"];
    assert_eq!(d["user"]["email"], "ada@example.com");
    assert_ne!(d["accessToken"].as_str(), Some(s.access_token.as_str()));
    assert_ne!(d["refreshToken"].as_str(), s.refresh_token.as_deref());
    let new_cookie = res.set_cookie().unwrap();
    assert_ne!(new_cookie.split(';').next(), s.cookie.split(';').next());
    assert!(new_cookie.contains("HttpOnly") && new_cookie.contains("Max-Age=604800"));
    // The new access token works, and so does the new refresh token.
    let me = t
        .send(t.get("/auth/me").bearer(d["accessToken"].as_str().unwrap()))
        .await;
    assert_eq!(me.status, StatusCode::OK);
    let again = refresh(&t, new_cookie.split(';').next().unwrap()).await;
    assert_eq!(again.status, StatusCode::OK);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn refresh_accepts_the_header_for_cross_origin_setups(pool: PgPool) {
    let t = TestApp::new(pool).await;
    t.user("ada@example.com", Role::Agent).await;
    let s = t.signin("ada@example.com").await;
    let res = t
        .send(
            t.post("/auth/refresh")
                .header("x-refresh-token", s.refresh_token.as_deref().unwrap()),
        )
        .await;
    assert_eq!(res.status, StatusCode::OK);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn refresh_without_a_token_is_token_missing(pool: PgPool) {
    let t = TestApp::new(pool).await;
    let res = t.send(t.post("/auth/refresh")).await;
    assert_eq!(
        (res.status, res.error_code().as_str()),
        (StatusCode::UNAUTHORIZED, "TOKEN_MISSING")
    );
    let res = t
        .send(t.post("/auth/refresh").cookie("refresh_token=; other=1"))
        .await;
    assert_eq!(res.error_code(), "TOKEN_MISSING");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn refresh_with_unknown_or_garbage_token_expires_and_clears_the_cookie(pool: PgPool) {
    let t = TestApp::new(pool).await;
    for token in ["nonsense", &"A".repeat(43), &"B".repeat(500)] {
        let res = refresh(&t, &format!("refresh_token={token}")).await;
        assert_eq!(
            (res.status, res.error_code().as_str()),
            (StatusCode::UNAUTHORIZED, "TOKEN_EXPIRED")
        );
        assert!(res.set_cookie().unwrap().contains("Max-Age=0"));
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn parallel_tab_replay_within_grace_is_refused_but_keeps_the_family(pool: PgPool) {
    let t = TestApp::new(pool).await;
    t.user("ada@example.com", Role::Agent).await;
    let s = t.signin("ada@example.com").await;
    let winner = refresh(&t, &s.cookie_pair()).await;
    assert_eq!(winner.status, StatusCode::OK);

    let loser = refresh(&t, &s.cookie_pair()).await;
    assert_eq!(
        (loser.status, loser.error_code().as_str()),
        (StatusCode::UNAUTHORIZED, "TOKEN_EXPIRED")
    );
    // The cookie the winning tab just received must survive.
    assert!(loser.set_cookie().is_none());
    let family = family_of(&t, "ada@example.com").await;
    assert!(
        sessions::get_family(&t.pool, family)
            .await
            .unwrap()
            .unwrap()
            .revoked_at
            .is_none()
    );
    let next = winner.set_cookie().unwrap();
    assert_eq!(
        refresh(&t, next.split(';').next().unwrap()).await.status,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reuse_after_the_grace_window_revokes_the_family(pool: PgPool) {
    let t = TestApp::new(pool).await;
    t.user("ada@example.com", Role::Agent).await;
    let s = t.signin("ada@example.com").await;
    let winner = refresh(&t, &s.cookie_pair()).await;
    let newest = winner.set_cookie().unwrap();
    let newest = newest.split(';').next().unwrap().to_string();

    sqlx::query("UPDATE refresh_tokens SET rotated_at = now() - interval '11 seconds' WHERE rotated_at IS NOT NULL")
        .execute(&t.pool)
        .await
        .unwrap();
    let thief = refresh(&t, &s.cookie_pair()).await;
    assert_eq!(
        (thief.status, thief.error_code().as_str()),
        (StatusCode::UNAUTHORIZED, "TOKEN_EXPIRED")
    );
    assert!(thief.set_cookie().unwrap().contains("Max-Age=0"));

    let family = sessions::get_family(&t.pool, family_of(&t, "ada@example.com").await)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(family.revoked_reason.as_deref(), Some("reuse_detected"));
    // The legitimate newest token died with the family.
    assert_eq!(refresh(&t, &newest).await.error_code(), "TOKEN_EXPIRED");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn family_lives_at_most_90_days(pool: PgPool) {
    let t = TestApp::new(pool).await;
    t.user("ada@example.com", Role::Agent).await;
    let s = t.signin("ada@example.com").await;
    let (days, token_days): (f64, f64) = sqlx::query_as(
        "SELECT (extract(epoch FROM f.expires_at - now()) / 86400)::float8, (extract(epoch FROM t.expires_at - now()) / 86400)::float8
         FROM refresh_families f JOIN refresh_tokens t ON t.family_id = f.id",
    )
    .fetch_one(&t.pool)
    .await
    .unwrap();
    assert!((89.9..=90.0).contains(&days), "{days}");
    assert!((6.9..=7.0).contains(&token_days), "{token_days}");

    // Past the cap, even a never-used token is dead.
    sqlx::query("UPDATE refresh_families SET expires_at = now() - interval '1 second'")
        .execute(&t.pool)
        .await
        .unwrap();
    let res = refresh(&t, &s.cookie_pair()).await;
    assert_eq!(
        (res.status, res.error_code().as_str()),
        (StatusCode::UNAUTHORIZED, "TOKEN_EXPIRED")
    );
    assert!(res.set_cookie().unwrap().contains("Max-Age=0"));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn rotation_near_the_cap_does_not_outlive_the_family(pool: PgPool) {
    let t = TestApp::new(pool).await;
    t.user("ada@example.com", Role::Agent).await;
    let s = t.signin("ada@example.com").await;
    sqlx::query("UPDATE refresh_families SET expires_at = now() + interval '1 day'")
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(refresh(&t, &s.cookie_pair()).await.status, StatusCode::OK);
    let over: i64 = sqlx::query_scalar("SELECT count(*) FROM refresh_tokens t JOIN refresh_families f ON f.id = t.family_id WHERE t.expires_at > f.expires_at AND t.rotated_at IS NULL")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(over, 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn refresh_for_a_deactivated_user_revokes_the_family(pool: PgPool) {
    let t = TestApp::new(pool).await;
    t.user("ada@example.com", Role::Agent).await;
    let s = t.signin("ada@example.com").await;
    sqlx::query("UPDATE users SET status = 'suspended'")
        .execute(&t.pool)
        .await
        .unwrap();
    let res = refresh(&t, &s.cookie_pair()).await;
    assert_eq!(
        (res.status, res.error_code().as_str()),
        (StatusCode::UNAUTHORIZED, "TOKEN_EXPIRED")
    );
    let family = sessions::get_family(&t.pool, family_of(&t, "ada@example.com").await)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(family.revoked_reason.as_deref(), Some("user_inactive"));
}

// ---- signout ------------------------------------------------------------------------------

#[sqlx::test(migrations = "../db/migrations")]
async fn signout_revokes_the_family_and_clears_the_cookie(pool: PgPool) {
    let t = TestApp::new(pool).await;
    t.user("ada@example.com", Role::Agent).await;
    let s = t.signin("ada@example.com").await;
    let res = t
        .send(t.post("/auth/signout").cookie(&s.cookie_pair()))
        .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.json()["success"], true);
    assert!(res.set_cookie().unwrap().contains("Max-Age=0"));
    assert_eq!(
        refresh(&t, &s.cookie_pair()).await.error_code(),
        "TOKEN_EXPIRED"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn signout_always_succeeds(pool: PgPool) {
    let t = TestApp::new(pool).await;
    assert_eq!(t.send(t.post("/auth/signout")).await.status, StatusCode::OK);
    assert_eq!(
        t.send(t.post("/auth/signout").cookie("refresh_token=unknown"))
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        t.send(
            t.post("/auth/signout")
                .cookie(&format!("refresh_token={}", "z".repeat(500)))
        )
        .await
        .status,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn signout_all_revokes_every_session_of_the_caller_only(pool: PgPool) {
    let t = TestApp::new(pool).await;
    t.user("ada@example.com", Role::Agent).await;
    t.user("bob@example.com", Role::Agent).await;
    let (a1, a2, b) = (
        t.signin("ada@example.com").await,
        t.signin("ada@example.com").await,
        t.signin("bob@example.com").await,
    );
    let res = t
        .send(t.post("/auth/signout-all").bearer(&a1.access_token))
        .await;
    assert_eq!(res.status, StatusCode::OK);
    assert!(res.set_cookie().unwrap().contains("Max-Age=0"));
    assert_eq!(
        refresh(&t, &a1.cookie_pair()).await.error_code(),
        "TOKEN_EXPIRED"
    );
    assert_eq!(
        refresh(&t, &a2.cookie_pair()).await.error_code(),
        "TOKEN_EXPIRED"
    );
    assert_eq!(refresh(&t, &b.cookie_pair()).await.status, StatusCode::OK);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn signout_all_needs_a_user(pool: PgPool) {
    let t = TestApp::new(pool).await;
    let res = t.send(t.post("/auth/signout-all")).await;
    assert_eq!(
        (res.status, res.error_code().as_str()),
        (StatusCode::UNAUTHORIZED, "TOKEN_MISSING")
    );
}

// ---- me / bearer --------------------------------------------------------------------------

#[sqlx::test(migrations = "../db/migrations")]
async fn me_error_codes(pool: PgPool) {
    let t = TestApp::new(pool).await;
    let user = t.user("ada@example.com", Role::Agent).await;
    let good = t.signin("ada@example.com").await.access_token;
    let code = |req: Req| async {
        let r = t.send(req).await;
        (
            r.status,
            r.error_code(),
            r.header("www-authenticate").map(String::from),
        )
    };

    let (st, c, h) = code(t.get("/auth/me")).await;
    assert_eq!(
        (st, c.as_str(), h.as_deref()),
        (StatusCode::UNAUTHORIZED, "TOKEN_MISSING", Some("Bearer"))
    );
    assert_eq!(
        code(t.get("/auth/me").bearer("garbage")).await.1,
        "TOKEN_INVALID"
    );
    assert_eq!(
        code(t.get("/auth/me").header("authorization", "Basic abc"))
            .await
            .1,
        "TOKEN_INVALID"
    );
    assert_eq!(
        code(t.get("/auth/me").header("authorization", &good))
            .await
            .1,
        "TOKEN_INVALID"
    );
    // Bearer scheme is case-insensitive.
    let ok = t
        .send(
            t.get("/auth/me")
                .header("authorization", &format!("bearer {good}")),
        )
        .await;
    assert_eq!(ok.status, StatusCode::OK);

    // Expired.
    let past = t
        .state
        .jwt
        .issue(user.id, chrono::Utc::now().timestamp() - 600, 300);
    assert_eq!(
        code(t.get("/auth/me").bearer(&past)).await.1,
        "TOKEN_EXPIRED"
    );
    // Signed by someone else.
    let other = api::jwt::Jwt::new(b"a-completely-different-32-byte-key!").issue(
        user.id,
        chrono::Utc::now().timestamp(),
        300,
    );
    assert_eq!(
        code(t.get("/auth/me").bearer(&other)).await.1,
        "TOKEN_INVALID"
    );
    // Tampered payload: swap in another subject but keep the signature.
    let parts: Vec<&str> = good.split('.').collect();
    let forged = t
        .state
        .jwt
        .issue(Uuid::new_v4(), chrono::Utc::now().timestamp(), 300);
    let forged_payload = forged.split('.').nth(1).unwrap();
    let tampered = format!("{}.{forged_payload}.{}", parts[0], parts[2]);
    assert_eq!(
        code(t.get("/auth/me").bearer(&tampered)).await.1,
        "TOKEN_INVALID"
    );
    // alg=none.
    use base64::Engine;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let none = format!(
        "{}.{}.",
        b64.encode(r#"{"alg":"none","typ":"JWT"}"#),
        parts[1]
    );
    assert_eq!(
        code(t.get("/auth/me").bearer(&none)).await.1,
        "TOKEN_INVALID"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn token_for_a_deleted_or_inactive_user_is_invalid(pool: PgPool) {
    let t = TestApp::new(pool).await;
    let user = t.user("ada@example.com", Role::Agent).await;
    let token = t.signin("ada@example.com").await.access_token;
    sqlx::query("UPDATE users SET status = 'inactive'")
        .execute(&t.pool)
        .await
        .unwrap();
    assert_eq!(
        t.send(t.get("/auth/me").bearer(&token)).await.error_code(),
        "TOKEN_INVALID"
    );
    users::delete(&t.pool, user.id).await.unwrap();
    assert_eq!(
        t.send(t.get("/auth/me").bearer(&token)).await.error_code(),
        "TOKEN_INVALID"
    );
}

// ---- bootstrap ----------------------------------------------------------------------------

#[sqlx::test(migrations = "../db/migrations")]
async fn bootstrap_creates_an_admin_who_can_sign_in(pool: PgPool) {
    let t = TestApp::new(pool).await;
    let u = create_admin(
        &t.pool,
        &t.state.hasher,
        " Root@Example.com ",
        PASSWORD,
        "Root",
    )
    .await
    .unwrap();
    assert_eq!(
        (u.email.as_str(), u.role),
        ("root@example.com", Role::Admin)
    );
    let s = t.signin("root@example.com").await;
    let me = t.send(t.get("/auth/me").bearer(&s.access_token)).await;
    assert_eq!(me.json()["data"]["user"]["role"], "admin");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn bootstrap_validates_and_refuses_duplicates(pool: PgPool) {
    let t = TestApp::new(pool).await;
    let h: &Hasher = &t.state.hasher;
    let e = create_admin(&t.pool, h, "bad", "short", "")
        .await
        .unwrap_err();
    assert_eq!(e.code(), "VALIDATION_ERROR");
    create_admin(&t.pool, h, "root@example.com", PASSWORD, "Root")
        .await
        .unwrap();
    let e = create_admin(&t.pool, h, "ROOT@example.com", PASSWORD, "Root")
        .await
        .unwrap_err();
    assert_eq!(e.code(), "CONFLICT");
}
