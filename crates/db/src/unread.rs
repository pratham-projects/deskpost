use chrono::{DateTime, Utc};
use sqlx::FromRow;

use crate::{Id, Pool, Result, new_id, tickets};

#[derive(Debug, Clone, FromRow)]
pub struct Unread {
    pub id: Id,
    pub ticket_id: Id,
    pub project_id: Id,
    pub created_at: DateTime<Utc>,
}

/// Unread tickets the actor can see, oldest first, optionally for one project.
pub async fn list(pool: &Pool, actor: Id, project_id: Option<Id>) -> Result<Vec<Unread>> {
    sqlx::query_as(&format!(
        "SELECT u.id, u.ticket_id, u.project_id, u.created_at
         FROM unread_tickets u JOIN tickets t ON t.id = u.ticket_id {}
         WHERE ($2::uuid IS NULL OR u.project_id = $2) ORDER BY u.created_at, u.id",
        tickets::VISIBLE
    ))
    .bind(actor)
    .bind(project_id)
    .fetch_all(pool)
    .await
}

/// System side; idempotent (at most one row per ticket). `false` if the ticket is not in the project.
pub async fn mark(pool: &Pool, project_id: Id, ticket_id: Id) -> Result<bool> {
    let r = sqlx::query(
        "INSERT INTO unread_tickets (id, project_id, ticket_id)
         SELECT $1, t.project_id, t.id FROM tickets t WHERE t.id = $3 AND t.project_id = $2
         ON CONFLICT (ticket_id) DO NOTHING",
    )
    .bind(new_id())
    .bind(project_id)
    .bind(ticket_id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() == 1)
}

/// Opening a ticket clears its marker for everyone. Returns the number of rows removed.
pub async fn clear(pool: &Pool, actor: Id, ticket_id: Id) -> Result<u64> {
    let r = sqlx::query(&format!(
        "DELETE FROM unread_tickets u USING tickets t {}
         WHERE u.ticket_id = t.id AND t.id = $2",
        tickets::VISIBLE
    ))
    .bind(actor)
    .bind(ticket_id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}
