//! `/users` (`docs/API.md` section 5). Account management is global-admin only; everyone can
//! read co-members and edit their own profile and password.

use axum::Router;
use axum::extract::State;
use axum::routing::{get, post};
use dp_core::email_addr::is_valid_email;
use dp_db::users::{self, AdminPatch, Guarded, ProfilePatch, UserFilter};
use dp_db::{Id, Role, Status, is_foreign_key_violation, is_unique_violation, sessions};
use serde::Deserialize;
use serde_json::{Value, json};

use super::qparams::Params;
use crate::client_ip::ClientIp;
use crate::dto::UserDto;
use crate::error::{ApiError, Envelope, Fields, Pagination};
use crate::extract::{ApiJson, ApiPath, AuthUser, GlobalAdmin};
use crate::ratelimit::Rule;
use crate::state::AppState;

const MAX_PASSWORD_CHARS: usize = 128;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users", get(list).post(create))
        .route("/users/me", get(me).patch(update_me))
        .route("/users/me/password", post(change_password))
        .route("/users/{id}", get(get_one).patch(update).delete(remove))
}

fn name(f: &mut Fields, n: &str) -> String {
    let n = n.trim().to_string();
    if n.is_empty() || n.chars().count() > 255 || n.chars().any(char::is_control) {
        f.add("name", "Name must be 1 to 255 characters.");
    }
    n
}

fn phone(f: &mut Fields, p: &str) -> String {
    let p = p.trim().to_string();
    if p.chars().count() > 20 || p.chars().any(char::is_control) {
        f.add("phone", "Phone must be at most 20 characters.");
    }
    p
}

fn timezone(f: &mut Fields, t: &str) -> String {
    let t = t.trim().to_string();
    if t.is_empty() || t.len() > 64 || t.chars().any(char::is_control) {
        f.add("timezone", "Timezone must be 1 to 64 characters.");
    }
    t
}

fn new_password(f: &mut Fields, field: &str, p: &str) {
    if !(8..=MAX_PASSWORD_CHARS).contains(&p.chars().count()) {
        f.add(field, "Password must be 8 to 128 characters.");
    }
}

async fn me(AuthUser(user): AuthUser) -> Envelope<UserDto> {
    Envelope::ok((&user).into())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ProfileReq {
    name: Option<String>,
    phone: Option<String>,
    timezone: Option<String>,
}

async fn update_me(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiJson(req): ApiJson<ProfileReq>,
) -> Result<Envelope<UserDto>, ApiError> {
    let mut f = Fields::default();
    let patch = ProfilePatch {
        name: req.name.map(|n| name(&mut f, &n)),
        phone: req.phone.map(|p| phone(&mut f, &p)),
        timezone: req.timezone.map(|t| timezone(&mut f, &t)),
    };
    f.finish()?;
    let u = users::update_profile(&s.pool, user.id, &patch)
        .await?
        .ok_or(ApiError::TokenInvalid)?;
    Ok(Envelope::ok((&u).into()))
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct PasswordReq {
    current_password: String,
    new_password: String,
}

/// Needs the current password, is rate limited (a stolen access token must not be a
/// password oracle) and signs every session out.
async fn change_password(
    State(s): State<AppState>,
    ClientIp(ip): ClientIp,
    AuthUser(user): AuthUser,
    ApiJson(req): ApiJson<PasswordReq>,
) -> Result<Envelope<Value>, ApiError> {
    s.limiter.check(Rule::PasswordChange, ip)?;
    let mut f = Fields::default();
    if req.current_password.is_empty() || req.current_password.chars().count() > MAX_PASSWORD_CHARS
    {
        f.add("currentPassword", "Current password is required.");
    }
    new_password(&mut f, "newPassword", &req.new_password);
    f.finish()?;
    if !s
        .hasher
        .verify(req.current_password, Some(user.password_hash.clone()))
        .await
    {
        return Err(ApiError::Validation(
            [(
                "currentPassword".to_string(),
                "Current password is incorrect.".to_string(),
            )]
            .into(),
        ));
    }
    let hash = s.hasher.hash(req.new_password).await?;
    users::set_password_hash(&s.pool, user.id, &hash).await?;
    sessions::revoke_all_for_user(&s.pool, user.id, "logout_all").await?;
    Ok(Envelope::ok(json!({ "message": "Password changed." })))
}

async fn list(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    q: Params,
) -> Result<Envelope<Vec<UserDto>>, ApiError> {
    let mut f = Fields::default();
    let page = q.page(&mut f);
    let role = q.get("role").map(|r| {
        r.parse::<Role>().unwrap_or_else(|_| {
            f.add("role", "Must be admin or agent.");
            Role::Agent
        })
    });
    let status = q.get("status").map(|v| {
        v.parse::<Status>().unwrap_or_else(|_| {
            f.add("status", "Must be active, inactive or suspended.");
            Status::Active
        })
    });
    f.finish()?;
    let (rows, total) = users::list(&s.pool, user.id, &UserFilter { role, status }, page).await?;
    Ok(
        Envelope::ok(rows.iter().map(UserDto::from).collect()).paginated(Pagination::new(
            total,
            page.limit,
            page.offset,
        )),
    )
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CreateReq {
    email: String,
    password: String,
    name: String,
    role: Option<String>,
}

fn role_of(f: &mut Fields, r: &str) -> Role {
    r.parse().unwrap_or_else(|_| {
        f.add("role", "Must be admin or agent.");
        Role::Agent
    })
}

async fn create(
    State(s): State<AppState>,
    GlobalAdmin(_): GlobalAdmin,
    ApiJson(req): ApiJson<CreateReq>,
) -> Result<Envelope<UserDto>, ApiError> {
    let mut f = Fields::default();
    let email = req.email.trim().to_ascii_lowercase();
    if !is_valid_email(&email) {
        f.add("email", "Enter a valid email address.");
    }
    new_password(&mut f, "password", &req.password);
    let n = name(&mut f, &req.name);
    let role = req.role.map_or(Role::Agent, |r| role_of(&mut f, &r));
    f.finish()?;
    let hash = s.hasher.hash(req.password).await?;
    let u = users::create(&s.pool, &email, &n, &hash, role)
        .await
        .map_err(|e| {
            if is_unique_violation(&e) {
                ApiError::Conflict("An account with this email already exists.".into())
            } else {
                e.into()
            }
        })?;
    Ok(Envelope::created((&u).into()))
}

async fn get_one(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(id): ApiPath<Id>,
) -> Result<Envelope<UserDto>, ApiError> {
    let u = users::get_visible(&s.pool, user.id, id)
        .await?
        .ok_or_else(|| ApiError::not_found("User"))?;
    Ok(Envelope::ok((&u).into()))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AdminReq {
    name: Option<String>,
    role: Option<String>,
    status: Option<String>,
    phone: Option<String>,
    timezone: Option<String>,
}

async fn update(
    State(s): State<AppState>,
    GlobalAdmin(_): GlobalAdmin,
    ApiPath(id): ApiPath<Id>,
    ApiJson(req): ApiJson<AdminReq>,
) -> Result<Envelope<UserDto>, ApiError> {
    let mut f = Fields::default();
    let patch = AdminPatch {
        name: req.name.map(|n| name(&mut f, &n)),
        role: req.role.map(|r| role_of(&mut f, &r)),
        status: req.status.map(|v| {
            v.parse::<Status>().unwrap_or_else(|_| {
                f.add("status", "Must be active, inactive or suspended.");
                Status::Active
            })
        }),
        phone: req.phone.map(|p| phone(&mut f, &p)),
        timezone: req.timezone.map(|t| timezone(&mut f, &t)),
    };
    f.finish()?;
    match users::admin_update_guarded(&s.pool, id, &patch).await? {
        Guarded::LastAdmin => Err(last_admin()),
        Guarded::Done(None) => Err(ApiError::not_found("User")),
        Guarded::Done(Some(u)) => Ok(Envelope::ok((&u).into())),
    }
}

fn last_admin() -> ApiError {
    ApiError::Conflict("At least one active administrator must remain.".into())
}

async fn remove(
    State(s): State<AppState>,
    GlobalAdmin(_): GlobalAdmin,
    ApiPath(id): ApiPath<Id>,
) -> Result<Envelope<Value>, ApiError> {
    match users::delete_guarded(&s.pool, id).await {
        Ok(Guarded::LastAdmin) => Err(last_admin()),
        Ok(Guarded::Done(false)) => Err(ApiError::not_found("User")),
        Ok(Guarded::Done(true)) => Ok(Envelope::ok(json!({ "success": true, "id": id }))),
        Err(e) if is_foreign_key_violation(&e) => Err(ApiError::Conflict(
            "This user still owns a project. Delete the project first.".into(),
        )),
        Err(e) => Err(e.into()),
    }
}
