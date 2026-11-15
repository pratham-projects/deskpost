//! Shared fixture for API tests: an app built with `build_app_with` (no socket), cheap Argon2,
//! request/response helpers and the tenant-extractor fixture routes.
//!
//! ```ignore
//! #[sqlx::test(migrations = "../db/migrations")]
//! async fn my_route(pool: PgPool) {
//!     let t = TestApp::new(pool).await;
//!     let (owner, outsider) = (t.user("o@example.com", Role::Agent).await, t.user("x@example.com", Role::Agent).await);
//!     let project = t.project(owner.id, "acme").await;
//!     let token = t.signin(&owner.email).await.access_token;
//!     let res = t.send(t.get(&format!("/projects/{}", project.id)).bearer(&token)).await;
//!     assert_eq!(res.status, 200);
//! }
//! ```
#![allow(dead_code)]

use std::net::SocketAddr;

use api::config::{Argon2Cost, Config};
use api::error::ApiError;
use api::extract::{ApiPath, AuthUser, GlobalAdmin, project_admin, project_role};
use api::{AppState, build_app_with};
use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, DefaultBodyLimit};
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::routing::{get, put};
use dp_db::projects::{NewProject, Project};
use dp_db::users::User;
use dp_db::{Id, Pool, Role, members, projects, users};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

pub const PASSWORD: &str = "correct-horse-1";
pub const JWT_SECRET: &str = "k3J9xQ2vL8mZ5nB7cR4wT6yU1aS0dFgH";
pub const SECRET_KEY: &str = "Zp7Qm2Xv9Lk4Nb8Tc3Wr6Yh1Gs5Df0Ja";
pub const FRONTEND: &str = "http://localhost:3001";

pub fn env_with(extra: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let mut vars: Vec<(String, String)> = [
        ("DATABASE_URL", "postgres://unused"),
        ("JWT_SECRET", JWT_SECRET),
        ("DP_SECRET_KEY", SECRET_KEY),
        ("DP_ENV", "development"),
        ("FRONTEND_URL", FRONTEND),
        ("DP_TRUSTED_PROXIES", "10.0.0.0/8"),
    ]
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .into();
    for (k, v) in extra {
        vars.retain(|(key, _)| key != k);
        vars.push((k.to_string(), v.to_string()));
    }
    move |k| {
        vars.iter()
            .find(|(key, _)| key == k)
            .map(|(_, v)| v.clone())
    }
}

pub fn test_config() -> Config {
    let mut c = Config::from_lookup(env_with(&[])).expect("test config is valid");
    c.allow_signup = true;
    c.first_user_admin = true;
    c.argon2 = Argon2Cost {
        memory_kib: 8,
        iterations: 1,
        parallelism: 1,
    };
    c
}

pub struct TestApp {
    pub app: Router,
    pub state: AppState,
    pub pool: Pool,
}

impl TestApp {
    pub async fn new(pool: Pool) -> Self {
        Self::with_config(pool, test_config()).await
    }

    pub async fn with_config(pool: Pool, config: Config) -> Self {
        let state = AppState::new(pool.clone(), config).expect("state");
        let app = build_app_with(state.clone(), fixture_routes());
        Self { app, state, pool }
    }

    pub fn version(&self) -> &str {
        &self.state.config.api_version
    }

    /// Request builders take a path under `/api/{version}`.
    pub fn req(&self, method: Method, path: &str) -> Req {
        Req(Request::builder()
            .method(method)
            .uri(format!("/api/{}{path}", self.version())))
    }

    pub fn get(&self, path: &str) -> Req {
        self.req(Method::GET, path)
    }

    pub fn post(&self, path: &str) -> Req {
        self.req(Method::POST, path)
    }

    pub async fn send(&self, req: Req) -> Resp {
        self.send_body(req, Body::empty()).await
    }

    pub async fn send_json(&self, req: Req, body: Value) -> Resp {
        self.send_body(
            req.header(header::CONTENT_TYPE, "application/json"),
            Body::from(body.to_string()),
        )
        .await
    }

    pub async fn send_body(&self, req: Req, body: Body) -> Resp {
        let mut req = req.0.body(body).unwrap();
        if req.extensions().get::<ConnectInfo<SocketAddr>>().is_none() {
            req.extensions_mut()
                .insert(ConnectInfo("192.0.2.1:4000".parse::<SocketAddr>().unwrap()));
        }
        let res = self.app.clone().oneshot(req).await.unwrap();
        let (parts, body) = res.into_parts();
        Resp {
            status: parts.status,
            headers: parts.headers,
            body: body.collect().await.unwrap().to_bytes().to_vec(),
        }
    }

    pub async fn user(&self, email: &str, role: Role) -> User {
        let hash = self.state.hasher.hash(PASSWORD.into()).await.unwrap();
        users::create(&self.pool, email, "Test User", &hash, role)
            .await
            .unwrap()
    }

    pub async fn signin(&self, email: &str) -> Session {
        let res = self
            .send_json(
                self.post("/auth/signin"),
                json!({ "email": email, "password": PASSWORD }),
            )
            .await;
        assert_eq!(res.status, StatusCode::OK, "signin: {}", res.text());
        Session {
            access_token: res.json()["data"]["accessToken"].as_str().unwrap().into(),
            refresh_token: res.json()["data"]["refreshToken"].as_str().map(Into::into),
            cookie: res.set_cookie().unwrap(),
        }
    }

    pub async fn project(&self, owner: Id, slug: &str) -> Project {
        projects::create(
            &self.pool,
            owner,
            &NewProject {
                name: format!("Project {slug}"),
                slug: slug.into(),
                domain: None,
                description: None,
                api_key_hash: format!("hash-{slug}"),
                api_key_prefix: "pk_test".into(),
            },
        )
        .await
        .unwrap()
    }

    pub async fn add_member(&self, admin: Id, project: Id, user: Id, role: Role) {
        members::add(&self.pool, admin, project, user, role, false)
            .await
            .unwrap()
            .expect("admin can add members");
    }
}

pub struct Session {
    pub access_token: String,
    /// Only present outside production.
    pub refresh_token: Option<String>,
    /// Full `Set-Cookie` header value.
    pub cookie: String,
}

impl Session {
    /// `refresh_token=...` pair to send back as a `Cookie` header.
    pub fn cookie_pair(&self) -> String {
        self.cookie.split(';').next().unwrap().to_string()
    }
}

pub struct Req(pub axum::http::request::Builder);

impl Req {
    pub fn header(self, k: impl TryInto<header::HeaderName>, v: &str) -> Self {
        Self(self.0.header(k.try_into().ok().unwrap(), v))
    }

    pub fn bearer(self, token: &str) -> Self {
        self.header(header::AUTHORIZATION, &format!("Bearer {token}"))
    }

    pub fn cookie(self, pair: &str) -> Self {
        self.header(header::COOKIE, pair)
    }

    /// Pretend the TCP peer is `ip` (to test trusted proxies).
    pub fn peer(self, ip: &str) -> Self {
        let addr: SocketAddr = format!("{ip}:4000").parse().unwrap();
        Self(self.0.extension(ConnectInfo(addr)))
    }
}

pub struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|_| panic!("not json: {}", self.text()))
    }

    pub fn error_code(&self) -> String {
        let j = self.json();
        assert_eq!(j["success"], false, "{j}");
        j["error"]["code"].as_str().expect("error.code").to_string()
    }

    pub fn set_cookie(&self) -> Option<String> {
        self.headers
            .get(header::SET_COOKIE)
            .map(|v| v.to_str().unwrap().to_string())
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(|v| v.to_str().unwrap())
    }
}

/// Routes mounted under `/api/{version}/_fixture` in every test app. Copy the shape for a new
/// route: pick the extractor, call the access helper, return `Envelope`.
fn fixture_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/_fixture/whoami",
            get(|AuthUser(u): AuthUser| async move { u.email }),
        )
        .route(
            "/_fixture/global-admin",
            get(|_: GlobalAdmin| async { "ok" }),
        )
        .route(
            "/_fixture/projects/{id}",
            get(
                |axum::extract::State(s): axum::extract::State<AppState>,
                 AuthUser(u): AuthUser,
                 ApiPath(id): ApiPath<Id>| async move {
                    let role = project_role(&s, u.id, id).await?;
                    Ok::<_, ApiError>(role.as_str())
                },
            )
            .delete(
                |axum::extract::State(s): axum::extract::State<AppState>,
                 AuthUser(u): AuthUser,
                 ApiPath(id): ApiPath<Id>| async move {
                    project_admin(&s, u.id, id).await?;
                    Ok::<_, ApiError>("deleted")
                },
            ),
        )
        // An upload-style route raises the body cap for itself only.
        .route(
            "/_fixture/upload",
            put(|body: axum::body::Bytes| async move { body.len().to_string() })
                .layer(DefaultBodyLimit::max(1024 * 1024)),
        )
        .route(
            "/_fixture/small",
            put(|body: axum::body::Bytes| async move { body.len().to_string() }),
        )
}
