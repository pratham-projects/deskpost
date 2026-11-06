//! `/auth/*` (`docs/API.md` section 4).

use axum::Router;
use axum::extract::State;
use axum::http::header::{COOKIE, SET_COOKIE, USER_AGENT};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use chrono::{Duration, Utc};
use sd_core::api_key::hash_api_key;
use sd_core::email_addr::is_valid_email;
use sd_db::sessions::{self, ClientMeta, Rotation};
use sd_db::users::{self, User};
use sd_db::{Role, Status, is_unique_violation};
use serde::{Deserialize, Serialize};

use crate::client_ip::ClientIp;
use crate::dto::UserDto;
use crate::error::{ApiError, Envelope, Fields};
use crate::extract::{ApiJson, AuthUser};
use crate::password::Hasher;
use crate::ratelimit::Rule;
use crate::state::AppState;

const ACCESS_TTL_SECS: i64 = 300;
const REFRESH_TTL_DAYS: i64 = 7;
const FAMILY_TTL_DAYS: i64 = 90;
/// A rotated token replayed within this window is a parallel tab, not theft.
const REFRESH_GRACE_SECS: i64 = 10;
const COOKIE_NAME: &str = "refresh_token";
const MAX_PASSWORD_CHARS: usize = 128;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/signup", post(signup))
        .route("/signin", post(signin))
        .route("/refresh", post(refresh))
        .route("/signout", post(signout))
        .route("/signout-all", post(signout_all))
        .route("/me", get(me))
        .route("/config", get(config))
}

#[derive(Deserialize)]
struct SignupReq {
    #[serde(default)]
    email: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct SigninReq {
    #[serde(default)]
    email: String,
    #[serde(default)]
    password: String,
}

/// Normalised, validated sign-up fields: `(lower-cased email, name)`.
fn validate_account(email: &str, password: &str, name: &str) -> Result<(String, String), ApiError> {
    let mut f = Fields::default();
    let email = email.trim().to_ascii_lowercase();
    if !is_valid_email(&email) {
        f.add("email", "Enter a valid email address.");
    }
    if !(8..=MAX_PASSWORD_CHARS).contains(&password.chars().count()) {
        f.add("password", "Password must be 8 to 128 characters.");
    }
    let name = name.trim().to_string();
    if name.is_empty() || name.chars().count() > 255 {
        f.add("name", "Name must be 1 to 255 characters.");
    }
    f.finish()?;
    Ok((email, name))
}

async fn signup(
    State(s): State<AppState>,
    ClientIp(ip): ClientIp,
    ApiJson(req): ApiJson<SignupReq>,
) -> Result<Response, ApiError> {
    s.limiter.check(Rule::Register, ip)?;
    if !s.config.allow_signup {
        return Err(ApiError::Forbidden("Sign-up is disabled.".into()));
    }
    let (email, name) = validate_account(&req.email, &req.password, &req.name)?;
    // Hash before touching the table so a taken email costs the same as a free one.
    let hash = s.hasher.hash(req.password).await?;
    let user = users::register(&s.pool, &email, &name, &hash, s.config.first_user_admin)
        .await
        .map_err(conflict_on_email)?;
    Ok(Envelope::created(serde_json::json!({
        "id": user.id, "email": user.email, "name": user.name, "role": user.role.as_str(),
    }))
    .into_response())
}

/// First admin from the CLI, before anyone can sign up (or when sign-up is disabled).
pub async fn create_admin(
    pool: &sd_db::Pool,
    hasher: &Hasher,
    email: &str,
    password: &str,
    name: &str,
) -> Result<User, ApiError> {
    let (email, name) = validate_account(email, password, name)?;
    let hash = hasher.hash(password.to_string()).await?;
    users::create(pool, &email, &name, &hash, Role::Admin)
        .await
        .map_err(conflict_on_email)
}

fn conflict_on_email(e: sd_db::Error) -> ApiError {
    if is_unique_violation(&e) {
        ApiError::Conflict("An account with this email already exists.".into())
    } else {
        e.into()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Session {
    user: UserDto,
    access_token: String,
    expires_in: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
}

async fn signin(
    State(s): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    ApiJson(req): ApiJson<SigninReq>,
) -> Result<Response, ApiError> {
    s.limiter.check(Rule::Login, ip)?;
    let mut f = Fields::default();
    if req.email.trim().is_empty() || req.email.len() > 254 {
        f.add("email", "Email is required.");
    }
    if req.password.is_empty() || req.password.chars().count() > MAX_PASSWORD_CHARS {
        f.add("password", "Password is required.");
    }
    f.finish()?;
    s.limiter.check_name(Rule::LoginAccount, req.email.trim())?;

    let found = users::get_by_email(&s.pool, req.email.trim()).await?;
    let matched = s
        .hasher
        .verify(
            req.password,
            found.as_ref().map(|u| u.password_hash.clone()),
        )
        .await;
    // One failure for unknown email, wrong password and inactive account alike.
    let user = found
        .filter(|u| matched && u.status == Status::Active)
        .ok_or(ApiError::InvalidCredentials)?;

    let token = new_refresh_token();
    let now = Utc::now();
    sessions::start_family(
        &s.pool,
        user.id,
        &token.hash,
        now + Duration::days(REFRESH_TTL_DAYS),
        now + Duration::days(FAMILY_TTL_DAYS),
        &meta(&headers, ip),
    )
    .await?;
    users::touch_login(&s.pool, user.id).await?;
    Ok(session_response(&s, &user, token.value))
}

async fn refresh(
    State(s): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    s.limiter.check(Rule::Refresh, ip)?;
    let presented = refresh_token_from(&headers).ok_or(ApiError::TokenMissing)?;
    if presented.len() > 128 {
        return Ok(expired(&s, true));
    }
    let next = new_refresh_token();
    let rotation = sessions::rotate(
        &s.pool,
        &hash_api_key(&presented),
        &next.hash,
        Utc::now() + Duration::days(REFRESH_TTL_DAYS),
        Duration::seconds(REFRESH_GRACE_SECS),
        &meta(&headers, ip),
    )
    .await?;
    match rotation {
        Rotation::Rotated { user_id, family_id } => {
            let active = users::get(&s.pool, user_id)
                .await?
                .filter(|u| u.status == Status::Active);
            let Some(user) = active else {
                sessions::revoke_family(&s.pool, family_id, "user_inactive").await?;
                return Ok(expired(&s, true));
            };
            Ok(session_response(&s, &user, next.value))
        }
        // The winning tab just stored a new cookie; clearing it here would log that tab out.
        Rotation::Grace => Ok(expired(&s, false)),
        Rotation::Reused | Rotation::Invalid => Ok(expired(&s, true)),
    }
}

async fn signout(State(s): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    if let Some(token) = refresh_token_from(&headers).filter(|t| t.len() <= 128) {
        sessions::revoke_by_token(&s.pool, &hash_api_key(&token)).await?;
    }
    Ok(signed_out(&s))
}

async fn signout_all(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Response, ApiError> {
    sessions::revoke_all_for_user(&s.pool, user.id, "logout_all").await?;
    Ok(signed_out(&s))
}

async fn me(AuthUser(user): AuthUser) -> Envelope<serde_json::Value> {
    Envelope::ok(serde_json::json!({ "user": UserDto::from(&user) }))
}

async fn config(State(s): State<AppState>) -> Envelope<serde_json::Value> {
    Envelope::ok(serde_json::json!({ "signupEnabled": s.config.allow_signup }))
}

struct RefreshToken {
    value: String,
    hash: String,
}

/// 256 random bits; only the SHA-256 is stored.
fn new_refresh_token() -> RefreshToken {
    use base64::Engine;
    let mut raw = [0u8; 32];
    rand::fill(&mut raw);
    let value = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
    RefreshToken {
        hash: hash_api_key(&value),
        value,
    }
}

fn session_response(s: &AppState, user: &User, refresh: String) -> Response {
    let access = s
        .jwt
        .issue(user.id, Utc::now().timestamp(), ACCESS_TTL_SECS);
    let cookie = cookie(s, &refresh, REFRESH_TTL_DAYS * 86_400);
    // Cross-origin dev setups cannot use the cookie; production never exposes the token to JS.
    let refresh_token = (!s.config.production).then_some(refresh);
    let body = Envelope::ok(Session {
        user: user.into(),
        access_token: access,
        expires_in: ACCESS_TTL_SECS,
        refresh_token,
    });
    ([(SET_COOKIE, cookie)], body).into_response()
}

fn signed_out(s: &AppState) -> Response {
    (
        [(SET_COOKIE, cookie(s, "", 0))],
        Envelope::ok(serde_json::json!({ "message": "Signed out." })),
    )
        .into_response()
}

fn expired(s: &AppState, clear_cookie: bool) -> Response {
    let mut res = ApiError::TokenExpired.into_response();
    if clear_cookie {
        res.headers_mut().append(SET_COOKIE, cookie(s, "", 0));
    }
    res
}

fn cookie(s: &AppState, value: &str, max_age: i64) -> HeaderValue {
    let c = &s.config;
    let same_site = if c.production { "Strict" } else { "Lax" };
    let secure = if c.cookie_secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{COOKIE_NAME}={value}; HttpOnly; Path=/api/{}/auth; SameSite={same_site}; Max-Age={max_age}{secure}",
        c.api_version
    ))
    .expect("cookie is built from ASCII parts")
}

/// Cookie first, then `X-Refresh-Token` (cross-origin dev setups).
fn refresh_token_from(headers: &HeaderMap) -> Option<String> {
    let from_cookie = headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, v)| *k == COOKIE_NAME && !v.is_empty())
        .map(|(_, v)| v.to_string());
    from_cookie.or_else(|| {
        headers
            .get("x-refresh-token")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(String::from)
    })
}

fn meta(headers: &HeaderMap, ip: std::net::IpAddr) -> ClientMeta {
    ClientMeta {
        ip_address: Some(ip.to_string()),
        user_agent: headers
            .get(USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.chars().take(255).collect()),
    }
}
