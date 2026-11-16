//! Ticket presence: a heartbeat row per (ticket, user), considered live for `ttl` seconds.

use sqlx::FromRow;

use crate::{Id, Pool, Result, tickets};

#[derive(Debug, Clone, FromRow)]
pub struct Viewer {
    pub user_id: Id,
    pub name: String,
}

/// Register/refresh the actor's heartbeat. `false` when the ticket is not visible.
pub async fn touch(pool: &Pool, actor: Id, ticket_id: Id) -> Result<bool> {
    let mut tx = pool.begin().await?;
    if tickets::visible_project(&mut tx, actor, ticket_id)
        .await?
        .is_none()
    {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO active_ticket_viewers (ticket_id, user_id) VALUES ($1, $2)
         ON CONFLICT (ticket_id, user_id) DO UPDATE SET last_seen_at = now()",
    )
    .bind(ticket_id)
    .bind(actor)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

pub async fn remove(pool: &Pool, actor: Id, ticket_id: Id) -> Result<bool> {
    let r = sqlx::query("DELETE FROM active_ticket_viewers WHERE ticket_id = $1 AND user_id = $2")
        .bind(ticket_id)
        .bind(actor)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() == 1)
}

/// Live viewers (seen within `ttl_secs`), oldest heartbeat first. `None` when the ticket is not visible.
pub async fn list(
    pool: &Pool,
    actor: Id,
    ticket_id: Id,
    ttl_secs: i64,
) -> Result<Option<Vec<Viewer>>> {
    let mut conn = pool.acquire().await?;
    if tickets::visible_project(&mut conn, actor, ticket_id)
        .await?
        .is_none()
    {
        return Ok(None);
    }
    let rows = sqlx::query_as(
        "SELECT v.user_id, u.name FROM active_ticket_viewers v JOIN users u ON u.id = v.user_id
         WHERE v.ticket_id = $1 AND v.last_seen_at > now() - make_interval(secs => $2)
         ORDER BY v.last_seen_at, v.user_id",
    )
    .bind(ticket_id)
    .bind(ttl_secs as f64)
    .fetch_all(&mut *conn)
    .await?;
    Ok(Some(rows))
}

/// Background GC of stale heartbeats.
pub async fn purge_stale(pool: &Pool, ttl_secs: i64) -> Result<u64> {
    let r = sqlx::query(
        "DELETE FROM active_ticket_viewers WHERE last_seen_at <= now() - make_interval(secs => $1)",
    )
    .bind(ttl_secs as f64)
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}
