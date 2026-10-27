use chrono::{DateTime, Utc};
use sqlx::FromRow;

use crate::{Id, Page, Pool, Result, new_id};

#[derive(Debug, Clone, FromRow)]
pub struct Template {
    pub id: Id,
    pub project_id: Id,
    pub title: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "t.id, t.project_id, t.title, t.body, t.created_at, t.updated_at";
const MEMBER: &str = "EXISTS (SELECT 1 FROM project_members m
    WHERE m.project_id = t.project_id AND m.user_id = $1)";
/// Project admins, or members granted `can_manage_templates`.
const MANAGER: &str = "EXISTS (SELECT 1 FROM project_members m
    WHERE m.project_id = t.project_id AND m.user_id = $1 AND (m.role = 'admin' OR m.can_manage_templates))";

/// Members only; `q` filters title/body case-insensitively. Ordered by title.
pub async fn list(
    pool: &Pool,
    actor: Id,
    project_id: Id,
    q: Option<&str>,
    page: Page,
) -> Result<(Vec<Template>, i64)> {
    let cond = format!(
        "WHERE t.project_id = $2 AND {MEMBER} AND ($3::text IS NULL OR t.title ILIKE $3 OR t.body ILIKE $3)"
    );
    let pattern = q.filter(|q| !q.is_empty()).map(crate::like_pattern);
    let total = sqlx::query_scalar(&format!("SELECT count(*) FROM templates t {cond}"))
        .bind(actor)
        .bind(project_id)
        .bind(&pattern)
        .fetch_one(pool)
        .await?;
    let rows = sqlx::query_as(&format!(
        "SELECT {COLS} FROM templates t {cond} ORDER BY lower(t.title), t.id LIMIT $4 OFFSET $5"
    ))
    .bind(actor)
    .bind(project_id)
    .bind(&pattern)
    .bind(page.limit)
    .bind(page.offset)
    .fetch_all(pool)
    .await?;
    Ok((rows, total))
}

pub async fn get(pool: &Pool, actor: Id, id: Id) -> Result<Option<Template>> {
    sqlx::query_as(&format!(
        "SELECT {COLS} FROM templates t WHERE t.id = $2 AND {MEMBER}"
    ))
    .bind(actor)
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn create(
    pool: &Pool,
    actor: Id,
    project_id: Id,
    title: &str,
    body: &str,
) -> Result<Option<Template>> {
    sqlx::query_as(
        "INSERT INTO templates AS t (id, project_id, title, body)
         SELECT $3, $2, $4, $5 WHERE EXISTS (SELECT 1 FROM project_members m
            WHERE m.project_id = $2 AND m.user_id = $1 AND (m.role = 'admin' OR m.can_manage_templates))
         RETURNING t.id, t.project_id, t.title, t.body, t.created_at, t.updated_at",
    )
    .bind(actor)
    .bind(project_id)
    .bind(new_id())
    .bind(title)
    .bind(body)
    .fetch_optional(pool)
    .await
}

pub async fn update(
    pool: &Pool,
    actor: Id,
    id: Id,
    title: Option<&str>,
    body: Option<&str>,
) -> Result<Option<Template>> {
    sqlx::query_as(&format!(
        "UPDATE templates t SET title = COALESCE($3, title), body = COALESCE($4, body), updated_at = now()
         WHERE t.id = $2 AND {MANAGER} RETURNING {COLS}"
    ))
    .bind(actor)
    .bind(id)
    .bind(title)
    .bind(body)
    .fetch_optional(pool)
    .await
}

pub async fn delete(pool: &Pool, actor: Id, id: Id) -> Result<bool> {
    let r = sqlx::query(&format!(
        "DELETE FROM templates t WHERE t.id = $2 AND {MANAGER}"
    ))
    .bind(actor)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() == 1)
}
