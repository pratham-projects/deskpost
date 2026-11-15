mod common;

use axum::body::Body;
use axum::http::{Method, StatusCode};
use common::*;
use serde_json::json;
use sqlx::PgPool;

// ---- health -------------------------------------------------------------------------------

#[sqlx::test(migrations = "../db/migrations")]
async fn health_endpoints(pool: PgPool) {
    let t = TestApp::new(pool).await;
    let live = t.send(t.get("/").header("x", "y")).await; // versioned root is not a route
    assert_eq!(live.status, StatusCode::NOT_FOUND);

    let res = send_root(&t, Method::GET, "/health").await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(
        res.json(),
        json!({ "success": true, "data": { "status": "ok" } })
    );

    let res = send_root(&t, Method::GET, "/health/db").await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(
        res.json(),
        json!({ "success": true, "data": { "status": "ok", "database": "up" } })
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn health_db_reports_down_without_details(pool: PgPool) {
    let t = TestApp::new(pool).await;
    t.pool.close().await;
    let res = send_root(&t, Method::GET, "/health/db").await;
    assert_eq!(res.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        res.json()["data"],
        json!({ "status": "error", "database": "down" })
    );
    // Liveness does not depend on the database.
    assert_eq!(
        send_root(&t, Method::GET, "/health").await.status,
        StatusCode::OK
    );
}

async fn send_root(t: &TestApp, method: Method, path: &str) -> Resp {
    send_root_with(t, axum::http::Request::builder().method(method).uri(path)).await
}

async fn send_root_with(t: &TestApp, b: axum::http::request::Builder) -> Resp {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let res = t
        .app
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let (parts, body) = res.into_parts();
    Resp {
        status: parts.status,
        headers: parts.headers,
        body: body.collect().await.unwrap().to_bytes().to_vec(),
    }
}

// ---- errors never leak --------------------------------------------------------------------

#[sqlx::test(migrations = "../db/migrations")]
async fn unknown_routes_and_methods_use_the_envelope(pool: PgPool) {
    let t = TestApp::new(pool).await;
    let res = t.send(t.get("/nope")).await;
    assert_eq!(
        (res.status, res.error_code().as_str()),
        (StatusCode::NOT_FOUND, "NOT_FOUND")
    );
    let res = send_root(&t, Method::GET, "/elsewhere").await;
    assert_eq!(res.error_code(), "NOT_FOUND");
    let res = t.send(t.get("/auth/signin")).await;
    assert_eq!(
        (res.status, res.error_code().as_str()),
        (StatusCode::METHOD_NOT_ALLOWED, "METHOD_NOT_ALLOWED")
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn database_failure_is_a_generic_500(pool: PgPool) {
    let t = TestApp::new(pool).await;
    t.user("ada@example.com", dp_db::Role::Agent).await;
    t.pool.close().await;
    let res = t
        .send_json(
            t.post("/auth/signin"),
            json!({ "email": "ada@example.com", "password": PASSWORD }),
        )
        .await;
    assert_eq!(
        (res.status, res.error_code().as_str()),
        (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL")
    );
    let body = res.text().to_lowercase();
    for leak in ["sqlx", "pool", "postgres", "select", "closed", "users"] {
        assert!(!body.contains(leak), "{leak} leaked: {body}");
    }
    assert_eq!(res.json()["error"]["message"], "Internal server error.");
}

// ---- body limits --------------------------------------------------------------------------

#[sqlx::test(migrations = "../db/migrations")]
async fn default_body_limit_is_64kb(pool: PgPool) {
    let t = TestApp::new(pool).await;
    let big = json!({ "email": "a@example.com", "password": "x".repeat(70 * 1024) });
    let res = t.send_json(t.post("/auth/signin"), big).await;
    assert_eq!(
        (res.status, res.error_code().as_str()),
        (StatusCode::PAYLOAD_TOO_LARGE, "PAYLOAD_TOO_LARGE")
    );

    let fits = json!({ "email": "a@example.com", "password": "x".repeat(60 * 1024) });
    let res = t.send_json(t.post("/auth/signin"), fits).await;
    assert_ne!(res.status, StatusCode::PAYLOAD_TOO_LARGE); // reaches validation (password > 128 chars)
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn body_limit_is_configurable_and_per_route(pool: PgPool) {
    let t = TestApp::new(pool).await;
    let put = |path: &'static str, n: usize| {
        let t = &t;
        async move {
            t.send_body(
                t.req(Method::PUT, path).bearer("x"),
                Body::from(vec![b'a'; n]),
            )
            .await
        }
    };
    // 500 KB: the global cap rejects it, the upload route's own cap allows it.
    let res = put("/_fixture/small", 500 * 1024).await;
    assert_eq!(res.status, StatusCode::PAYLOAD_TOO_LARGE);
    let res = put("/_fixture/upload", 500 * 1024).await;
    assert_eq!(
        (res.status, res.text().as_str()),
        (StatusCode::OK, "512000")
    );
    assert_eq!(
        put("/_fixture/upload", 2 * 1024 * 1024).await.status,
        StatusCode::PAYLOAD_TOO_LARGE
    );

    let mut c = test_config();
    c.body_limit = 100;
    let t2 = TestApp::with_config(t.pool.clone(), c).await;
    let res = t2
        .send_body(
            t2.req(Method::PUT, "/_fixture/small"),
            Body::from(vec![b'a'; 101]),
        )
        .await;
    assert_eq!(res.status, StatusCode::PAYLOAD_TOO_LARGE);
}

// ---- CORS ---------------------------------------------------------------------------------

#[sqlx::test(migrations = "../db/migrations")]
async fn preflight_from_the_frontend_is_allowed_with_credentials(pool: PgPool) {
    let t = TestApp::new(pool).await;
    let res = t
        .send(
            t.req(Method::OPTIONS, "/auth/signin")
                .header("origin", FRONTEND)
                .header("access-control-request-method", "POST")
                .header(
                    "access-control-request-headers",
                    "content-type,authorization,x-refresh-token",
                ),
        )
        .await;
    assert!(res.status.is_success(), "{}", res.status);
    assert_eq!(res.header("access-control-allow-origin"), Some(FRONTEND));
    assert_eq!(res.header("access-control-allow-credentials"), Some("true"));
    let methods = res.header("access-control-allow-methods").unwrap();
    assert!(
        methods.contains("POST") && methods.contains("PATCH") && methods.contains("DELETE"),
        "{methods}"
    );
    let headers = res
        .header("access-control-allow-headers")
        .unwrap()
        .to_lowercase();
    for h in [
        "content-type",
        "authorization",
        "x-refresh-token",
        "x-project-key",
        "x-webhook-signature",
    ] {
        assert!(headers.contains(h), "{h} in {headers}");
    }
    assert!(
        res.header("vary")
            .unwrap()
            .to_lowercase()
            .contains("origin")
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn other_origins_get_no_cors_headers(pool: PgPool) {
    let t = TestApp::new(pool).await;
    for origin in [
        "https://evil.example",
        "http://localhost:3002",
        "null",
        "http://localhost:3001.evil.example",
    ] {
        let res = t
            .send(
                t.req(Method::OPTIONS, "/auth/signin")
                    .header("origin", origin)
                    .header("access-control-request-method", "POST"),
            )
            .await;
        assert!(
            res.header("access-control-allow-origin").is_none(),
            "{origin}"
        );
        let res = t.send(t.get("/auth/config").header("origin", origin)).await;
        assert!(
            res.header("access-control-allow-origin").is_none(),
            "{origin}"
        );
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn actual_responses_carry_cors_and_error_responses_too(pool: PgPool) {
    let t = TestApp::new(pool).await;
    for path in ["/auth/config", "/auth/me", "/nope"] {
        let res = t.send(t.get(path).header("origin", FRONTEND)).await;
        assert_eq!(
            res.header("access-control-allow-origin"),
            Some(FRONTEND),
            "{path}"
        );
        assert_eq!(
            res.header("access-control-allow-credentials"),
            Some("true"),
            "{path}"
        );
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn no_frontend_url_means_no_cross_origin_access(pool: PgPool) {
    let c = api::config::Config::from_lookup(|k| env_with(&[])(k).filter(|_| k != "FRONTEND_URL"))
        .unwrap();
    let t = TestApp::with_config(pool, c).await;
    let res = t
        .send(t.get("/auth/config").header("origin", FRONTEND))
        .await;
    assert!(res.header("access-control-allow-origin").is_none());
}

// ---- security headers ---------------------------------------------------------------------

#[sqlx::test(migrations = "../db/migrations")]
async fn security_headers_on_every_response(pool: PgPool) {
    let t = TestApp::new(pool).await;
    for res in [
        t.send(t.get("/auth/config")).await,
        t.send(t.get("/nope")).await,
        send_root(&t, Method::GET, "/health").await,
    ] {
        assert_eq!(res.header("x-content-type-options"), Some("nosniff"));
        assert_eq!(res.header("x-frame-options"), Some("DENY"));
        assert_eq!(res.header("referrer-policy"), Some("no-referrer"));
        assert_eq!(res.header("cache-control"), Some("no-store"));
        assert!(
            res.header("content-security-policy")
                .unwrap()
                .contains("frame-ancestors 'none'")
        );
        assert!(
            res.header("strict-transport-security").is_none(),
            "HSTS only over TLS deployments"
        );
    }
}
