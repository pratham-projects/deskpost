use chrono::{DateTime, Utc};
use sqlx::FromRow;

use crate::{Id, Page, Pool, Result, Role, new_id};

#[derive(Debug, Clone, FromRow)]
pub struct Member {
    pub id: Id,
    pub project_id: Id,
    pub user_id: Id,
    pub role: Role,
    pub should_notify: bool,
    pub can_view_all_tickets: bool,
    pub can_manage_templates: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub user_email: String,
    pub user_name: String,
}

const SELECT: &str = "SELECT m.id, m.project_id, m.user_id, m.role, m.should_notify,
    m.can_view_all_tickets, m.can_manage_templates, m.created_at, m.updated_at,
    u.email AS user_email, u.name AS user_name
    FROM project_members m JOIN users u ON u.id = m.user_id";

const ACTOR_IS_MEMBER: &str =
    "EXISTS (SELECT 1 FROM project_members a WHERE a.project_id = m.project_id AND a.user_id = $1)";
const ACTOR_IS_ADMIN: &str = "EXISTS (SELECT 1 FROM project_members a
    WHERE a.project_id = $2 AND a.user_id = $1 AND a.role = 'admin')";

#[derive(Debug, Default, Clone)]
pub struct MemberPatch {
    pub role: Option<Role>,
    pub should_notify: Option<bool>,
    pub can_view_all_tickets: Option<bool>,
    pub can_manage_templates: Option<bool>,
}

/// The actor's role in the project, `None` when they are not a member (or it does not exist).
pub async fn role_of(pool: &Pool, user_id: Id, project_id: Id) -> Result<Option<Role>> {
    sqlx::query_scalar("SELECT role FROM project_members WHERE project_id = $2 AND user_id = $1")
        .bind(user_id)
        .bind(project_id)
        .fetch_optional(pool)
        .await
}

/// Project-admin only. A second add of the same user violates the unique constraint (409).
pub async fn add(
    pool: &Pool,
    actor: Id,
    project_id: Id,
    user_id: Id,
    role: Role,
    should_notify: bool,
) -> Result<Option<Member>> {
    let id = new_id();
    let done = sqlx::query(&format!(
        "INSERT INTO project_members (id, project_id, user_id, role, should_notify)
         SELECT $3, $2, $4, $5, $6 WHERE {ACTOR_IS_ADMIN}"
    ))
    .bind(actor)
    .bind(project_id)
    .bind(id)
    .bind(user_id)
    .bind(role)
    .bind(should_notify)
    .execute(pool)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(None);
    }
    sqlx::query_as(&format!("{SELECT} WHERE m.id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn list(pool: &Pool, actor: Id, project_id: Id) -> Result<Vec<Member>> {
    sqlx::query_as(&format!(
        "{SELECT} WHERE m.project_id = $2 AND {ACTOR_IS_MEMBER} ORDER BY m.created_at, m.id"
    ))
    .bind(actor)
    .bind(project_id)
    .fetch_all(pool)
    .await
}

/// Case-insensitive substring match on member name or email; an empty `q` matches everyone.
pub async fn search(
    pool: &Pool,
    actor: Id,
    project_id: Id,
    q: &str,
    page: Page,
) -> Result<Vec<Member>> {
    sqlx::query_as(&format!(
        "{SELECT} WHERE m.project_id = $2 AND {ACTOR_IS_MEMBER}
           AND (u.name ILIKE $3 OR u.email ILIKE $3)
         ORDER BY u.name, m.id LIMIT $4 OFFSET $5"
    ))
    .bind(actor)
    .bind(project_id)
    .bind(crate::like_pattern(q))
    .bind(page.limit)
    .bind(page.offset)
    .fetch_all(pool)
    .await
}

/// Project-admin only. The owner cannot be demoted (a project always keeps an admin).
pub async fn update(
    pool: &Pool,
    actor: Id,
    project_id: Id,
    user_id: Id,
    patch: &MemberPatch,
) -> Result<Option<Member>> {
    let done = sqlx::query(&format!(
        "UPDATE project_members m SET role = COALESCE($4, role),
                should_notify = COALESCE($5, should_notify),
                can_view_all_tickets = COALESCE($6, can_view_all_tickets),
                can_manage_templates = COALESCE($7, can_manage_templates), updated_at = now()
         WHERE m.project_id = $2 AND m.user_id = $3 AND {ACTOR_IS_ADMIN}
           AND NOT ($4::text IS NOT NULL AND $4 <> 'admin'
                    AND EXISTS (SELECT 1 FROM projects p WHERE p.id = $2 AND p.owner_user_id = $3))"
    ))
    .bind(actor)
    .bind(project_id)
    .bind(user_id)
    .bind(patch.role)
    .bind(patch.should_notify)
    .bind(patch.can_view_all_tickets)
    .bind(patch.can_manage_templates)
    .execute(pool)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(None);
    }
    sqlx::query_as(&format!(
        "{SELECT} WHERE m.project_id = $1 AND m.user_id = $2"
    ))
    .bind(project_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
}

/// Project-admin, or a member removing themself. The owner cannot be removed. Tickets assigned to
/// the removed member become unassigned (FK `SET NULL`).
pub async fn remove(pool: &Pool, actor: Id, project_id: Id, user_id: Id) -> Result<bool> {
    let r = sqlx::query(&format!(
        "DELETE FROM project_members m WHERE m.project_id = $2 AND m.user_id = $3
           AND ($1 = $3 OR {ACTOR_IS_ADMIN})
           AND EXISTS (SELECT 1 FROM project_members a WHERE a.project_id = $2 AND a.user_id = $1)
           AND NOT EXISTS (SELECT 1 FROM projects p WHERE p.id = $2 AND p.owner_user_id = $3)"
    ))
    .bind(actor)
    .bind(project_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() == 1)
}

/// Members who asked for new-ticket notification mail (system side).
pub async fn notify_recipients(pool: &Pool, project_id: Id) -> Result<Vec<(String, String)>> {
    sqlx::query_as(
        "SELECT u.email, u.name FROM project_members m JOIN users u ON u.id = m.user_id
         WHERE m.project_id = $1 AND m.should_notify AND u.status = 'active' ORDER BY u.email",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await
}
