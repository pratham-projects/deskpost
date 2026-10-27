use chrono::{DateTime, Utc};
use sqlx::FromRow;

use crate::{Id, Page, Pool, Result, new_id};

#[derive(Debug, Clone, FromRow)]
pub struct Client {
    pub id: Id,
    pub project_id: Id,
    pub email: String,
    pub name: Option<String>,
    pub phone: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "c.id, c.project_id, c.email, c.name, c.phone, c.created_at, c.updated_at";
const ACTOR_IS_MEMBER: &str = "EXISTS (SELECT 1 FROM project_members m
    WHERE m.project_id = c.project_id AND m.user_id = $1)";

/// One client per (project, lower(email)). The first name seen wins; later calls only fill a missing name.
pub(crate) async fn get_or_create(
    conn: &mut sqlx::PgConnection,
    project_id: Id,
    email: &str,
    name: Option<&str>,
) -> Result<Client> {
    sqlx::query_as(
        "INSERT INTO clients AS c (id, project_id, email, name) VALUES ($1, $2, $3, $4)
         ON CONFLICT (project_id, lower(email))
         DO UPDATE SET name = COALESCE(c.name, EXCLUDED.name), updated_at = now()
         RETURNING c.id, c.project_id, c.email, c.name, c.phone, c.created_at, c.updated_at",
    )
    .bind(new_id())
    .bind(project_id)
    .bind(email)
    .bind(name)
    .fetch_one(conn)
    .await
}

/// System side (inbound mail): same semantics inside one project.
pub async fn upsert(
    pool: &Pool,
    project_id: Id,
    email: &str,
    name: Option<&str>,
) -> Result<Client> {
    let mut conn = pool.acquire().await?;
    get_or_create(&mut conn, project_id, email, name).await
}

pub async fn get(pool: &Pool, actor: Id, id: Id) -> Result<Option<Client>> {
    sqlx::query_as(&format!(
        "SELECT {COLS} FROM clients c WHERE c.id = $2 AND {ACTOR_IS_MEMBER}"
    ))
    .bind(actor)
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// Members of the project only; `q` matches email or name.
pub async fn list(
    pool: &Pool,
    actor: Id,
    project_id: Id,
    q: Option<&str>,
    page: Page,
) -> Result<Vec<Client>> {
    sqlx::query_as(&format!(
        "SELECT {COLS} FROM clients c
         WHERE c.project_id = $2 AND {ACTOR_IS_MEMBER}
           AND ($3::text IS NULL OR c.email ILIKE $3 OR c.name ILIKE $3)
         ORDER BY c.created_at, c.id LIMIT $4 OFFSET $5"
    ))
    .bind(actor)
    .bind(project_id)
    .bind(q.map(crate::like_pattern))
    .bind(page.limit)
    .bind(page.offset)
    .fetch_all(pool)
    .await
}
