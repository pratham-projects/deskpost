use chrono::{DateTime, Utc};
use sqlx::FromRow;

use crate::{Id, Pool, Result, new_id, tickets};

#[derive(Debug, Clone, FromRow)]
pub struct Attachment {
    pub id: Id,
    pub project_id: Id,
    pub ticket_id: Id,
    pub message_id: Option<Id>,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub storage_key: String,
    pub uploaded_by_user_id: Option<Id>,
    pub created_at: DateTime<Utc>,
}

const COLS: &str = "a.id, a.project_id, a.ticket_id, a.message_id, a.filename, a.mime_type,
    a.size_bytes, a.storage_key, a.uploaded_by_user_id, a.created_at";

#[derive(Debug, Clone)]
pub struct NewAttachment {
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    /// Opaque key from `sd-storage`; never a URL or a user-supplied path.
    pub storage_key: String,
    /// Inbound mail: the message the file arrived with (must belong to the same ticket).
    pub message_id: Option<Id>,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Stored {
    Ok(Attachment),
    /// Ticket (or message) missing or not visible to the uploader.
    NoAccess,
    /// Adding the file would exceed the project's `storage_quota_mb`.
    QuotaExceeded,
}

async fn store(pool: &Pool, scope: Scope, ticket_id: Id, new: &NewAttachment) -> Result<Stored> {
    let mut tx = pool.begin().await?;
    let project_id = match scope {
        Scope::Actor(actor) => tickets::visible_project(&mut tx, actor, ticket_id).await?,
        Scope::Project(project) => {
            sqlx::query_scalar("SELECT project_id FROM tickets WHERE id = $1 AND project_id = $2")
                .bind(ticket_id)
                .bind(project)
                .fetch_optional(&mut *tx)
                .await?
        }
    };
    let Some(project_id) = project_id else {
        return Ok(Stored::NoAccess);
    };
    if let Some(message_id) = new.message_id {
        let ok: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM messages WHERE id = $1 AND ticket_id = $2)",
        )
        .bind(message_id)
        .bind(ticket_id)
        .fetch_one(&mut *tx)
        .await?;
        if !ok {
            return Ok(Stored::NoAccess);
        }
    }
    // Locking the project row serialises concurrent uploads so the quota cannot be raced past.
    let (quota, used): (i64, i64) = sqlx::query_as(
        "SELECT p.storage_quota_mb::bigint * 1048576,
                COALESCE((SELECT sum(size_bytes) FROM attachments WHERE project_id = p.id), 0)::bigint
         FROM projects p WHERE p.id = $1 FOR UPDATE",
    )
    .bind(project_id)
    .fetch_one(&mut *tx)
    .await?;
    if used + new.size_bytes > quota {
        return Ok(Stored::QuotaExceeded);
    }
    let uploader = match scope {
        Scope::Actor(actor) => Some(actor),
        Scope::Project(_) => None,
    };
    let attachment = sqlx::query_as(&format!(
        "INSERT INTO attachments AS a (id, project_id, ticket_id, message_id, filename, mime_type,
                                       size_bytes, storage_key, uploaded_by_user_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         RETURNING {COLS}"
    ))
    .bind(new_id())
    .bind(project_id)
    .bind(ticket_id)
    .bind(new.message_id)
    .bind(&new.filename)
    .bind(&new.mime_type)
    .bind(new.size_bytes)
    .bind(&new.storage_key)
    .bind(uploader)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Stored::Ok(attachment))
}

#[derive(Clone, Copy)]
enum Scope {
    Actor(Id),
    Project(Id),
}

/// Agent upload to a visible ticket; the row starts unlinked until a message claims it.
pub async fn create(pool: &Pool, actor: Id, ticket_id: Id, new: &NewAttachment) -> Result<Stored> {
    store(pool, Scope::Actor(actor), ticket_id, new).await
}

/// System side (inbound mail): the ticket must belong to `project_id`.
pub async fn create_for_project(
    pool: &Pool,
    project_id: Id,
    ticket_id: Id,
    new: &NewAttachment,
) -> Result<Stored> {
    store(pool, Scope::Project(project_id), ticket_id, new).await
}

pub async fn get(pool: &Pool, actor: Id, id: Id) -> Result<Option<Attachment>> {
    sqlx::query_as(&format!(
        "SELECT {COLS} FROM attachments a JOIN tickets t ON t.id = a.ticket_id {} WHERE a.id = $2",
        tickets::VISIBLE
    ))
    .bind(actor)
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// `None` when the ticket is not visible.
pub async fn list_for_ticket(
    pool: &Pool,
    actor: Id,
    ticket_id: Id,
) -> Result<Option<Vec<Attachment>>> {
    let mut conn = pool.acquire().await?;
    if tickets::visible_project(&mut conn, actor, ticket_id)
        .await?
        .is_none()
    {
        return Ok(None);
    }
    let rows = sqlx::query_as(&format!(
        "SELECT {COLS} FROM attachments a WHERE a.ticket_id = $1 ORDER BY a.created_at, a.id"
    ))
    .bind(ticket_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(Some(rows))
}

/// Bytes stored for a project (system side, for display and tests).
pub async fn usage_bytes(pool: &Pool, project_id: Id) -> Result<i64> {
    sqlx::query_scalar(
        "SELECT COALESCE(sum(size_bytes), 0)::bigint FROM attachments WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_one(pool)
    .await
}

/// Files to remove from storage before deleting a ticket. Project-admin only; empty otherwise.
pub async fn storage_keys_for_ticket(pool: &Pool, actor: Id, ticket_id: Id) -> Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT a.storage_key FROM attachments a
         WHERE a.ticket_id = $2 AND EXISTS (
            SELECT 1 FROM project_members m
            WHERE m.project_id = a.project_id AND m.user_id = $1 AND m.role = 'admin')
         ORDER BY a.created_at, a.id",
    )
    .bind(actor)
    .bind(ticket_id)
    .fetch_all(pool)
    .await
}

/// Files to remove from storage before deleting a project. Owner only; empty otherwise.
pub async fn storage_keys_for_project(
    pool: &Pool,
    actor: Id,
    project_id: Id,
) -> Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT a.storage_key FROM attachments a JOIN projects p ON p.id = a.project_id
         WHERE a.project_id = $2 AND p.owner_user_id = $1 ORDER BY a.created_at, a.id",
    )
    .bind(actor)
    .bind(project_id)
    .fetch_all(pool)
    .await
}
