use chrono::{DateTime, Utc};
use sqlx::FromRow;

use crate::{Id, Page, Pool, Result, Role, Status, new_id};

#[derive(Debug, Clone, FromRow)]
pub struct User {
    pub id: Id,
    pub email: String,
    pub name: String,
    pub password_hash: String,
    pub role: Role,
    pub status: Status,
    pub avatar_url: Option<String>,
    pub phone: Option<String>,
    pub timezone: String,
    pub last_login_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "id, email, name, password_hash, role, status, avatar_url, phone, timezone, \
                    last_login_at, created_at, updated_at";

/// Users the caller may see: everything for a global admin, else self and project co-members.
const VISIBLE: &str = "(
    (SELECT role = 'admin' FROM users WHERE id = $1)
    OR u.id = $1
    OR EXISTS (SELECT 1 FROM project_members a JOIN project_members b ON a.project_id = b.project_id
               WHERE a.user_id = $1 AND b.user_id = u.id)
)";

#[derive(Debug, Default, Clone)]
pub struct ProfilePatch {
    pub name: Option<String>,
    /// `Some("")` clears the phone.
    pub phone: Option<String>,
    pub timezone: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct AdminPatch {
    pub name: Option<String>,
    pub role: Option<Role>,
    pub status: Option<Status>,
    pub phone: Option<String>,
    pub timezone: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct UserFilter {
    pub role: Option<Role>,
    pub status: Option<Status>,
}

pub async fn create(
    pool: &Pool,
    email: &str,
    name: &str,
    password_hash: &str,
    role: Role,
) -> Result<User> {
    sqlx::query_as(&format!(
        "INSERT INTO users (id, email, name, password_hash, role) VALUES ($1, $2, $3, $4, $5)
         RETURNING {COLS}"
    ))
    .bind(new_id())
    .bind(email)
    .bind(name)
    .bind(password_hash)
    .bind(role)
    .fetch_one(pool)
    .await
}

/// Self-service sign-up. The very first user becomes the global admin, everyone after is an agent.
pub async fn register(pool: &Pool, email: &str, name: &str, password_hash: &str) -> Result<User> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(7001)")
        .execute(&mut *tx)
        .await?;
    let user = sqlx::query_as(&format!(
        "INSERT INTO users (id, email, name, password_hash, role)
         VALUES ($1, $2, $3, $4, CASE WHEN EXISTS (SELECT 1 FROM users) THEN 'agent' ELSE 'admin' END)
         RETURNING {COLS}"
    ))
    .bind(new_id())
    .bind(email)
    .bind(name)
    .bind(password_hash)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(user)
}

pub async fn get(pool: &Pool, id: Id) -> Result<Option<User>> {
    sqlx::query_as(&format!("SELECT {COLS} FROM users WHERE id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// Case-insensitive lookup, used by sign-in.
pub async fn get_by_email(pool: &Pool, email: &str) -> Result<Option<User>> {
    sqlx::query_as(&format!(
        "SELECT {COLS} FROM users WHERE lower(email) = lower($1)"
    ))
    .bind(email)
    .fetch_optional(pool)
    .await
}

/// A user the actor is allowed to see (self, co-member, or any user for a global admin).
pub async fn get_visible(pool: &Pool, actor: Id, id: Id) -> Result<Option<User>> {
    sqlx::query_as(&format!(
        "SELECT {COLS} FROM users u WHERE u.id = $2 AND {VISIBLE}"
    ))
    .bind(actor)
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn list(
    pool: &Pool,
    actor: Id,
    filter: &UserFilter,
    page: Page,
) -> Result<(Vec<User>, i64)> {
    let cond = format!(
        "WHERE {VISIBLE} AND ($2::text IS NULL OR u.role = $2) AND ($3::text IS NULL OR u.status = $3)"
    );
    let total: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM users u {cond}"))
        .bind(actor)
        .bind(filter.role)
        .bind(filter.status)
        .fetch_one(pool)
        .await?;
    let rows = sqlx::query_as(&format!(
        "SELECT {COLS} FROM users u {cond} ORDER BY u.created_at, u.id LIMIT $4 OFFSET $5"
    ))
    .bind(actor)
    .bind(filter.role)
    .bind(filter.status)
    .bind(page.limit)
    .bind(page.offset)
    .fetch_all(pool)
    .await?;
    Ok((rows, total))
}

pub async fn touch_login(pool: &Pool, id: Id) -> Result<()> {
    sqlx::query("UPDATE users SET last_login_at = now() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_profile(pool: &Pool, id: Id, patch: &ProfilePatch) -> Result<Option<User>> {
    sqlx::query_as(&format!(
        "UPDATE users SET name = COALESCE($2, name), phone = NULLIF(COALESCE($3, phone), ''),
                timezone = COALESCE($4, timezone), updated_at = now()
         WHERE id = $1 RETURNING {COLS}"
    ))
    .bind(id)
    .bind(&patch.name)
    .bind(&patch.phone)
    .bind(&patch.timezone)
    .fetch_optional(pool)
    .await
}

/// Global-admin edit. The caller must have checked that the actor is a global admin.
pub async fn admin_update(pool: &Pool, id: Id, patch: &AdminPatch) -> Result<Option<User>> {
    sqlx::query_as(&format!(
        "UPDATE users SET name = COALESCE($2, name), role = COALESCE($3, role),
                status = COALESCE($4, status), phone = NULLIF(COALESCE($5, phone), ''),
                timezone = COALESCE($6, timezone), updated_at = now()
         WHERE id = $1 RETURNING {COLS}"
    ))
    .bind(id)
    .bind(&patch.name)
    .bind(patch.role)
    .bind(patch.status)
    .bind(&patch.phone)
    .bind(&patch.timezone)
    .fetch_optional(pool)
    .await
}

pub async fn set_password_hash(pool: &Pool, id: Id, password_hash: &str) -> Result<bool> {
    let r = sqlx::query("UPDATE users SET password_hash = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(password_hash)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() == 1)
}

/// Fails with a foreign-key violation while the user still owns a project.
pub async fn delete(pool: &Pool, id: Id) -> Result<bool> {
    let r = sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() == 1)
}
