use chrono::{DateTime, Utc};
use sqlx::FromRow;

use crate::{EmailLogStatus, Id, Pool, Result, new_id, tickets};

#[derive(Debug, Clone, FromRow)]
pub struct EmailLog {
    pub id: Id,
    pub project_id: Id,
    pub ticket_id: Id,
    pub message_id: Option<Id>,
    pub to_email: String,
    pub subject: Option<String>,
    pub sent_by_user_id: Option<Id>,
    pub smtp_message_id: Option<String>,
    pub status: EmailLogStatus,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "id, project_id, ticket_id, message_id, to_email, subject, sent_by_user_id,
    smtp_message_id, status, error, created_at, updated_at";

#[derive(Debug, Clone)]
pub struct NewEmailLog {
    pub project_id: Id,
    pub ticket_id: Id,
    pub message_id: Option<Id>,
    pub to_email: String,
    pub subject: Option<String>,
    pub sent_by_user_id: Option<Id>,
}

/// System side; starts as `pending`. The (project, ticket) pair must match (composite FK).
pub async fn create(pool: &Pool, new: &NewEmailLog) -> Result<EmailLog> {
    sqlx::query_as(&format!(
        "INSERT INTO email_logs (id, project_id, ticket_id, message_id, to_email, subject, sent_by_user_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {COLS}"
    ))
    .bind(new_id())
    .bind(new.project_id)
    .bind(new.ticket_id)
    .bind(new.message_id)
    .bind(&new.to_email)
    .bind(&new.subject)
    .bind(new.sent_by_user_id)
    .fetch_one(pool)
    .await
}

pub async fn mark_sent(pool: &Pool, id: Id, smtp_message_id: &str) -> Result<bool> {
    let r = sqlx::query(
        "UPDATE email_logs SET status = 'sent', smtp_message_id = $2, error = NULL, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(smtp_message_id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() == 1)
}

pub async fn mark_failed(pool: &Pool, id: Id, error: &str) -> Result<bool> {
    let r = sqlx::query(
        "UPDATE email_logs SET status = 'failed', error = $2, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() == 1)
}

/// Newest first. `None` when the ticket is not visible.
pub async fn list_for_ticket(
    pool: &Pool,
    actor: Id,
    ticket_id: Id,
) -> Result<Option<Vec<EmailLog>>> {
    let mut conn = pool.acquire().await?;
    if tickets::visible_project(&mut conn, actor, ticket_id)
        .await?
        .is_none()
    {
        return Ok(None);
    }
    let rows = sqlx::query_as(&format!(
        "SELECT {COLS} FROM email_logs WHERE ticket_id = $1 ORDER BY created_at DESC, id DESC"
    ))
    .bind(ticket_id)
    .fetch_all(&mut *conn)
    .await?;
    Ok(Some(rows))
}
