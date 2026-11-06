//! System-side queries for mail (R23): inbound ticket creation, threading lookups, the persisted
//! auto-response queue and per-project IMAP diagnostics. No `actor`: callers are the webhook (signed),
//! the IMAP worker and the outbox worker, which have already routed to a project.

use chrono::{DateTime, Utc};
use sqlx::FromRow;

use crate::attachments::{Attachment, COLS as ATT_COLS};
use crate::{Id, Pool, Result, clients, new_id};

/// A client's first email: becomes a ticket plus its first message in one transaction.
#[derive(Debug, Clone)]
pub struct NewInboundTicket {
    pub project_id: Id,
    pub client_email: String,
    pub client_name: Option<String>,
    pub subject: String,
    pub body: String,
    pub email_message_id: String,
    pub email_in_reply_to: Option<String>,
    pub email_references: Option<String>,
    pub email_from: Option<String>,
    pub email_to: Option<String>,
    pub email_received_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreatedTicket {
    pub ticket_id: Id,
    pub ticket_number: i64,
    pub message_id: Id,
}

/// `None` when the Message-ID is already stored for the project (replay): nothing is written and
/// no ticket number is consumed, because the whole transaction rolls back.
pub async fn create_inbound_ticket(
    pool: &Pool,
    new: &NewInboundTicket,
) -> Result<Option<CreatedTicket>> {
    let mut tx = pool.begin().await?;
    let client = clients::get_or_create(
        &mut tx,
        new.project_id,
        &new.client_email,
        new.client_name.as_deref(),
    )
    .await?;
    let number: Option<i64> = sqlx::query_scalar(
        "UPDATE projects SET next_ticket_number = next_ticket_number + 1 WHERE id = $1
         RETURNING next_ticket_number - 1",
    )
    .bind(new.project_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(number) = number else {
        return Ok(None);
    };
    let (ticket_id, message_id) = (new_id(), new_id());
    sqlx::query(
        "INSERT INTO tickets (id, project_id, ticket_number, client_id, subject, description, source)
         VALUES ($1, $2, $3, $4, $5, $6, 'email')",
    )
    .bind(ticket_id)
    .bind(new.project_id)
    .bind(number)
    .bind(client.id)
    .bind(&new.subject)
    .bind(&new.body)
    .execute(&mut *tx)
    .await?;
    let inserted = sqlx::query(
        "INSERT INTO messages (id, project_id, ticket_id, sender_type, sender_email, sender_name, body,
                               email_message_id, email_in_reply_to, email_references, email_from,
                               email_to, email_subject, email_received_at)
         VALUES ($1, $2, $3, 'client', $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
         ON CONFLICT (project_id, email_message_id) WHERE email_message_id IS NOT NULL DO NOTHING",
    )
    .bind(message_id)
    .bind(new.project_id)
    .bind(ticket_id)
    .bind(&client.email)
    .bind(&client.name)
    .bind(&new.body)
    .bind(&new.email_message_id)
    .bind(&new.email_in_reply_to)
    .bind(&new.email_references)
    .bind(&new.email_from)
    .bind(&new.email_to)
    .bind(&new.subject)
    .bind(new.email_received_at)
    .execute(&mut *tx)
    .await?;
    if inserted.rows_affected() == 0 {
        return Ok(None);
    }
    sqlx::query("INSERT INTO unread_tickets (id, project_id, ticket_id) VALUES ($1, $2, $3)")
        .bind(new_id())
        .bind(new.project_id)
        .bind(ticket_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(CreatedTicket {
        ticket_id,
        ticket_number: number,
        message_id,
    }))
}

/// The ticket (within the project) that owns any of these Message-IDs: a stored message, or an
/// auto-response we sent (the client may reply to that one first).
pub async fn find_ticket_by_refs(
    pool: &Pool,
    project_id: Id,
    message_ids: &[String],
) -> Result<Option<Id>> {
    sqlx::query_scalar(
        "SELECT ticket_id FROM (
            SELECT ticket_id, created_at FROM messages
             WHERE project_id = $1 AND email_message_id = ANY($2)
            UNION ALL
            SELECT ticket_id, created_at FROM email_logs
             WHERE project_id = $1 AND smtp_message_id = ANY($2)
         ) m ORDER BY created_at DESC LIMIT 1",
    )
    .bind(project_id)
    .bind(message_ids)
    .fetch_optional(pool)
    .await
}

/// Does the ticket exist in the project (a Message-ID we minted names a ticket id).
pub async fn ticket_in_project(pool: &Pool, project_id: Id, ticket_id: Id) -> Result<bool> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM tickets WHERE id = $1 AND project_id = $2)")
        .bind(ticket_id)
        .bind(project_id)
        .fetch_one(pool)
        .await
}

#[derive(Debug, Clone, FromRow)]
pub struct TicketMail {
    pub ticket_id: Id,
    pub ticket_number: i64,
    pub subject: String,
    pub description: Option<String>,
    pub status: String,
    pub client_email: String,
    pub client_name: Option<String>,
    pub project_name: String,
}

pub async fn ticket_mail(pool: &Pool, project_id: Id, ticket_id: Id) -> Result<Option<TicketMail>> {
    sqlx::query_as(
        "SELECT t.id AS ticket_id, t.ticket_number, t.subject, t.description, t.status,
                c.email AS client_email, c.name AS client_name, p.name AS project_name
         FROM tickets t JOIN clients c ON c.id = t.client_id JOIN projects p ON p.id = t.project_id
         WHERE t.id = $1 AND t.project_id = $2",
    )
    .bind(ticket_id)
    .bind(project_id)
    .fetch_optional(pool)
    .await
}

/// Message-IDs already used in the ticket's thread (stored messages and sent auto-responses),
/// oldest first, at most 50: the `References` chain of the next reply.
pub async fn thread_ids(pool: &Pool, project_id: Id, ticket_id: Id) -> Result<Vec<String>> {
    let mut ids: Vec<String> = sqlx::query_scalar(
        "SELECT mid FROM (
            SELECT email_message_id AS mid, created_at FROM messages
             WHERE project_id = $1 AND ticket_id = $2 AND email_message_id IS NOT NULL
            UNION ALL
            SELECT smtp_message_id, created_at FROM email_logs
             WHERE project_id = $1 AND ticket_id = $2 AND status = 'sent' AND smtp_message_id IS NOT NULL
         ) m ORDER BY created_at DESC LIMIT 50",
    )
    .bind(project_id)
    .bind(ticket_id)
    .fetch_all(pool)
    .await?;
    ids.reverse();
    Ok(ids)
}

/// Attachments of one ticket by id (to put on an outgoing mail). Rows of other tickets are not returned.
pub async fn attachments_of_ticket(
    pool: &Pool,
    project_id: Id,
    ticket_id: Id,
    ids: &[Id],
) -> Result<Vec<Attachment>> {
    sqlx::query_as(&format!(
        "SELECT {ATT_COLS} FROM attachments a
         WHERE a.project_id = $1 AND a.ticket_id = $2 AND a.id = ANY($3) ORDER BY a.created_at, a.id"
    ))
    .bind(project_id)
    .bind(ticket_id)
    .bind(ids)
    .fetch_all(pool)
    .await
}

/// Download by signed token: the token already proves an authorised member asked for this object.
pub async fn attachment_system(pool: &Pool, id: Id) -> Result<Option<Attachment>> {
    sqlx::query_as(&format!(
        "SELECT {ATT_COLS} FROM attachments a WHERE a.id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// Delete an attachment row the actor may remove (its uploader or a project admin, and the ticket
/// is visible to them). Returns the storage key to remove from disk.
pub async fn delete_attachment(pool: &Pool, actor: Id, id: Id) -> Result<Option<String>> {
    sqlx::query_scalar(
        "DELETE FROM attachments a USING tickets t, project_members m
         WHERE a.id = $2 AND t.id = a.ticket_id AND m.project_id = a.project_id AND m.user_id = $1
           AND (m.can_view_all_tickets OR t.assigned_to_user_id = m.user_id OR t.created_by_user_id = m.user_id)
           AND (m.role = 'admin' OR a.uploaded_by_user_id = m.user_id)
         RETURNING a.storage_key",
    )
    .bind(actor)
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// Roll back an upload that never became a message (system side, scoped to the ticket).
pub async fn delete_attachments(pool: &Pool, ticket_id: Id, ids: &[Id]) -> Result<()> {
    sqlx::query(
        "DELETE FROM attachments WHERE ticket_id = $1 AND id = ANY($2) AND message_id IS NULL",
    )
    .bind(ticket_id)
    .bind(ids)
    .execute(pool)
    .await?;
    Ok(())
}

// ---- auto-response queue ----

#[derive(Debug, Clone, FromRow)]
pub struct OutboxJob {
    pub id: Id,
    pub project_id: Id,
    pub ticket_id: Id,
    pub email_log_id: Id,
    pub kind: String,
    /// Attempts including the one just claimed.
    pub attempts: i32,
}

/// Pending rows one project may hold; beyond it new auto-responses are dropped, not queued.
pub const MAX_PENDING_PER_PROJECT: i64 = 1000;
/// Auto-responses to one address per hour and project; stops a spoofed sender being mail-bombed.
pub const MAX_PER_ADDRESS_HOUR: i64 = 5;

/// Queue one auto-response (and its `pending` email_logs row). `None` when a bound was hit.
pub async fn enqueue(
    pool: &Pool,
    project_id: Id,
    ticket_id: Id,
    kind: &str,
    to_email: &str,
    subject: &str,
) -> Result<Option<Id>> {
    let mut tx = pool.begin().await?;
    // Serialises per project so the bounds below cannot be raced past.
    sqlx::query("SELECT 1 FROM projects WHERE id = $1 FOR UPDATE")
        .bind(project_id)
        .execute(&mut *tx)
        .await?;
    let (pending, to_address): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE o.status = 'pending'),
                count(*) FILTER (WHERE lower(l.to_email) = lower($2)
                                 AND o.created_at > now() - interval '1 hour')
         FROM mail_outbox o JOIN email_logs l ON l.id = o.email_log_id WHERE o.project_id = $1",
    )
    .bind(project_id)
    .bind(to_email)
    .fetch_one(&mut *tx)
    .await?;
    if pending >= MAX_PENDING_PER_PROJECT || to_address >= MAX_PER_ADDRESS_HOUR {
        return Ok(None);
    }
    let (log_id, id) = (new_id(), new_id());
    sqlx::query(
        "INSERT INTO email_logs (id, project_id, ticket_id, to_email, subject) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(log_id)
    .bind(project_id)
    .bind(ticket_id)
    .bind(to_email)
    .bind(subject)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO mail_outbox (id, project_id, ticket_id, email_log_id, kind) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(project_id)
    .bind(ticket_id)
    .bind(log_id)
    .bind(kind)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(id))
}

/// Take up to `limit` due jobs. Each is leased for `lease_secs` (its `next_attempt_at` moves out and
/// `attempts` goes up), so a crashed worker's job comes back later and two workers never share one.
pub async fn claim_due(pool: &Pool, limit: i64, lease_secs: i64) -> Result<Vec<OutboxJob>> {
    sqlx::query_as(
        "UPDATE mail_outbox SET attempts = attempts + 1, updated_at = now(),
                next_attempt_at = now() + make_interval(secs => $2)
         WHERE id IN (SELECT id FROM mail_outbox WHERE status = 'pending' AND next_attempt_at <= now()
                      ORDER BY next_attempt_at, id LIMIT $1 FOR UPDATE SKIP LOCKED)
         RETURNING id, project_id, ticket_id, email_log_id, kind, attempts",
    )
    .bind(limit.clamp(1, 100))
    .bind(lease_secs as f64)
    .fetch_all(pool)
    .await
}

pub async fn mark_sent(pool: &Pool, job: &OutboxJob, smtp_message_id: &str) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE mail_outbox SET status = 'sent', last_error = NULL, updated_at = now() WHERE id = $1")
        .bind(job.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE email_logs SET status = 'sent', smtp_message_id = $2, error = NULL, updated_at = now() WHERE id = $1",
    )
    .bind(job.email_log_id)
    .bind(smtp_message_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// `retry_in_secs: Some(n)` schedules another attempt; `None` gives up (row `dead`, log `failed`).
pub async fn mark_failed(
    pool: &Pool,
    job: &OutboxJob,
    error: &str,
    retry_in_secs: Option<i64>,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    match retry_in_secs {
        Some(secs) => {
            sqlx::query(
                "UPDATE mail_outbox SET last_error = $2, updated_at = now(),
                        next_attempt_at = now() + make_interval(secs => $3)
                 WHERE id = $1",
            )
            .bind(job.id)
            .bind(error)
            .bind(secs as f64)
            .execute(&mut *tx)
            .await?;
        }
        None => {
            sqlx::query("UPDATE mail_outbox SET status = 'dead', last_error = $2, updated_at = now() WHERE id = $1")
                .bind(job.id)
                .bind(error)
                .execute(&mut *tx)
                .await?;
            sqlx::query(
                "UPDATE email_logs SET status = 'failed', error = $2, updated_at = now() WHERE id = $1",
            )
            .bind(job.email_log_id)
            .bind(error)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await
}

/// Housekeeping: finished rows older than `days` go away (the email_logs row stays).
pub async fn purge_finished(pool: &Pool, days: i32) -> Result<u64> {
    let r = sqlx::query(
        "DELETE FROM mail_outbox WHERE status <> 'pending' AND updated_at < now() - make_interval(days => $1)",
    )
    .bind(days)
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}

// ---- IMAP diagnostics ----

/// Record the outcome of a poll: `None` clears the last error. The caller strips secrets first.
pub async fn set_imap_status(pool: &Pool, project_id: Id, error: Option<&str>) -> Result<()> {
    sqlx::query(
        "UPDATE projects SET imap_last_error = $2, imap_last_polled_at = now() WHERE id = $1",
    )
    .bind(project_id)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn imap_status(
    pool: &Pool,
    project_id: Id,
) -> Result<Option<(Option<String>, Option<DateTime<Utc>>)>> {
    sqlx::query_as("SELECT imap_last_error, imap_last_polled_at FROM projects WHERE id = $1")
        .bind(project_id)
        .fetch_optional(pool)
        .await
}
