use chrono::{DateTime, Utc};
use sqlx::FromRow;

use crate::{EmailResponseStatus, Id, Pool, Result, new_id};

#[derive(Debug, Clone, FromRow)]
pub struct EmailResponse {
    pub id: Id,
    pub project_id: Id,
    pub status: EmailResponseStatus,
    pub subject: String,
    pub body: String,
    pub is_enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str =
    "r.id, r.project_id, r.status, r.subject, r.body, r.is_enabled, r.created_at, r.updated_at";
const MEMBER: &str = "EXISTS (SELECT 1 FROM project_members m
    WHERE m.project_id = r.project_id AND m.user_id = $1)";
const ADMIN: &str = "EXISTS (SELECT 1 FROM project_members m
    WHERE m.project_id = r.project_id AND m.user_id = $1 AND m.role = 'admin')";

pub async fn list(
    pool: &Pool,
    actor: Id,
    project_id: Id,
    status: Option<EmailResponseStatus>,
) -> Result<Vec<EmailResponse>> {
    sqlx::query_as(&format!(
        "SELECT {COLS} FROM email_responses r
         WHERE r.project_id = $2 AND {MEMBER} AND ($3::text IS NULL OR r.status = $3)
         ORDER BY r.status, r.id"
    ))
    .bind(actor)
    .bind(project_id)
    .bind(status)
    .fetch_all(pool)
    .await
}

/// Project-admin only. A second row for the same (project, status) violates the unique key.
pub async fn create(
    pool: &Pool,
    actor: Id,
    project_id: Id,
    status: EmailResponseStatus,
    subject: &str,
    body: &str,
    is_enabled: bool,
) -> Result<Option<EmailResponse>> {
    sqlx::query_as(
        "INSERT INTO email_responses AS r (id, project_id, status, subject, body, is_enabled)
         SELECT $3, $2, $4, $5, $6, $7 WHERE EXISTS (SELECT 1 FROM project_members m
            WHERE m.project_id = $2 AND m.user_id = $1 AND m.role = 'admin')
         RETURNING r.id, r.project_id, r.status, r.subject, r.body, r.is_enabled, r.created_at, r.updated_at",
    )
    .bind(actor)
    .bind(project_id)
    .bind(new_id())
    .bind(status)
    .bind(subject)
    .bind(body)
    .bind(is_enabled)
    .fetch_optional(pool)
    .await
}

/// Project-admin only. `subject: Some("")` clears the custom subject.
pub async fn update(
    pool: &Pool,
    actor: Id,
    id: Id,
    subject: Option<&str>,
    body: Option<&str>,
    is_enabled: Option<bool>,
) -> Result<Option<EmailResponse>> {
    sqlx::query_as(&format!(
        "UPDATE email_responses r SET subject = COALESCE($3, subject), body = COALESCE($4, body),
                is_enabled = COALESCE($5, is_enabled), updated_at = now()
         WHERE r.id = $2 AND {ADMIN} RETURNING {COLS}"
    ))
    .bind(actor)
    .bind(id)
    .bind(subject)
    .bind(body)
    .bind(is_enabled)
    .fetch_optional(pool)
    .await
}

pub async fn delete(pool: &Pool, actor: Id, id: Id) -> Result<bool> {
    let r = sqlx::query(&format!(
        "DELETE FROM email_responses r WHERE r.id = $2 AND {ADMIN}"
    ))
    .bind(actor)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() == 1)
}

/// System side: the enabled auto-reply for a ticket event, if the project configured one.
pub async fn get_enabled(
    pool: &Pool,
    project_id: Id,
    status: EmailResponseStatus,
) -> Result<Option<EmailResponse>> {
    sqlx::query_as(&format!(
        "SELECT {COLS} FROM email_responses r
         WHERE r.project_id = $1 AND r.status = $2 AND r.is_enabled"
    ))
    .bind(project_id)
    .bind(status)
    .fetch_optional(pool)
    .await
}

/// The project a response belongs to, for callers that must tell 404 from 403 before acting.
pub async fn project_of(pool: &Pool, id: Id) -> Result<Option<Id>> {
    sqlx::query_scalar("SELECT project_id FROM email_responses WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
}
