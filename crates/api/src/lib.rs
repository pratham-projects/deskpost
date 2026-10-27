//! SendDesk API. `build_app(state)` returns the whole service as a `Router`, so tests drive it with
//! `tower::ServiceExt::oneshot` and no socket.

use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, HeaderName, HeaderValue, REFERRER_POLICY, RETRY_AFTER,
    STRICT_TRANSPORT_SECURITY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use axum::http::{Method, Request};
use axum::routing::get;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

pub mod client_ip;
pub mod config;
pub mod dto;
pub mod error;
pub mod extract;
pub mod jwt;
pub mod password;
pub mod ratelimit;
pub mod routes;
pub mod state;

pub use state::AppState;

use error::ApiError;

const ALLOWED_HEADERS: [&str; 9] = [
    "content-type",
    "authorization",
    "x-refresh-token",
    "x-project-key",
    "x-filename",
    "x-message",
    "x-send-email",
    "x-webhook-signature",
    "x-webhook-timestamp",
];

pub fn build_app(state: AppState) -> Router {
    build_app_with(state, Router::new())
}

/// Like [`build_app`] with `extra` routes mounted under `/api/{version}`. Feature crates add their
/// routers in `routes`; tests use this to mount fixture routes.
pub fn build_app_with(state: AppState, extra: Router<AppState>) -> Router {
    let cfg = state.config.clone();
    let versioned = Router::new()
        .nest("/auth", routes::auth::router())
        .merge(routes::project_routes())
        .merge(extra);
    let origins: Vec<HeaderValue> = cfg
        .cors_origins
        .iter()
        .filter_map(|o| o.parse().ok())
        .collect();
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_credentials(true)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        .allow_headers(ALLOWED_HEADERS.map(HeaderName::from_static))
        .expose_headers([RETRY_AFTER])
        .max_age(Duration::from_secs(600));
    let header = |name: HeaderName, value: &'static str| {
        SetResponseHeaderLayer::if_not_present(name, HeaderValue::from_static(value))
    };

    let app = Router::new()
        .route("/health", get(routes::health::live))
        .route("/health/db", get(routes::health::db))
        .nest(&format!("/api/{}", cfg.api_version), versioned)
        .fallback(|| async { ApiError::NotFound("Route not found.".into()) })
        .method_not_allowed_fallback(|| async { ApiError::MethodNotAllowed })
        .layer(DefaultBodyLimit::max(cfg.body_limit))
        .layer(header(X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .layer(header(X_FRAME_OPTIONS, "DENY"))
        .layer(header(REFERRER_POLICY, "no-referrer"))
        .layer(header(
            CONTENT_SECURITY_POLICY,
            "default-src 'none'; frame-ancestors 'none'",
        ))
        .layer(header(CACHE_CONTROL, "no-store"))
        .layer(
            // Path only: query strings can carry download tokens.
            TraceLayer::new_for_http().make_span_with(|req: &Request<_>| {
                tracing::info_span!("http", method = %req.method(), path = %req.uri().path())
            }),
        )
        .layer(cors)
        .with_state(state);
    if cfg.cookie_secure {
        app.layer(header(
            STRICT_TRANSPORT_SECURITY,
            "max-age=31536000; includeSubDomains",
        ))
    } else {
        app
    }
}
