use chrono::{DateTime, Utc};
use sqlx::FromRow;

use crate::{Id, MessageType, Page, Pool, Result, SenderType, new_id, tickets};

#[derive(Debug, Clone, FromRow)]
pub struct Message {
    pub id: Id,
    pub project_id: Id,
    pub ticket_id: Id,
    pub sender_user_id: Option<Id>,
    pub sender_type: SenderType,
    pub sender_email: Option<String>,
    pub sender_name: Option<String>,
    pub body: String,
    pub html_body: Option<String>,
    pub message_type: MessageType,
    pub is_internal: bool,
    pub is_published: bool,
    pub email_message_id: Option<String>,
    pub email_in_reply_to: Option<String>,
    pub email_references: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Attachments linked to this message, oldest first.
    pub attachment_ids: Vec<Id>,
    /// Current account details of the sending agent (`None` for clients or deleted users).
    pub agent_email: Option<String>,
    pub agent_name: Option<String>,
}

const SELECT: &str = "SELECT m.id, m.project_id, m.ticket_id, m.sender_user_id, m.sender_type,
    m.sender_email, m.sender_name, m.body, m.html_body, m.message_type, m.is_internal, m.is_published,
    m.email_message_id, m.email_in_reply_to, m.email_references, m.created_at, m.updated_at,
    ARRAY(SELECT a.id FROM attachments a WHERE a.message_id = m.id ORDER BY a.created_at, a.id) AS attachment_ids,
    su.email AS agent_email, su.name AS agent_name
    FROM messages m LEFT JOIN users su ON su.id = m.sender_user_id";

#[derive(Debug, Default, Clone)]
pub struct NewAgentMessage {
    pub body: String,
    pub is_internal: bool,
    /// Unlinked attachments of the same ticket to attach to this message.
    pub attachment_ids: Vec<Id>,
    /// Set when the reply was emailed (the Message-ID we generated, for threading and dedup).
    pub email_message_id: Option<String>,
    pub email_in_reply_to: Option<String>,
    pub email_references: Option<String>,
    pub email_to: Option<String>,
    pub email_subject: Option<String>,
    /// Queue the reply for email in the same transaction (ignored for internal notes).
    pub outbox: Option<ReplyOutbox>,
}

/// Who and what an emailed reply is addressed to; recorded on the `email_logs` row at queue time.
#[derive(Debug, Clone)]
pub struct ReplyOutbox {
    pub to_email: String,
    pub subject: String,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Posted {
    Ok(Message),
    /// Ticket missing or not visible to the actor.
    NoAccess,
    /// An attachment id is unknown, from another ticket, or already linked.
    BadAttachments,
}

#[derive(Debug, Default, Clone)]
pub struct NewInbound {
    pub project_id: Id,
    pub ticket_id: Id,
    pub sender_email: String,
    pub sender_name: Option<String>,
    pub body: String,
    pub html_body: Option<String>,
    pub email_message_id: Option<String>,
    pub email_in_reply_to: Option<String>,
    pub email_references: Option<String>,
    pub email_from: Option<String>,
    pub email_to: Option<String>,
    pub email_subject: Option<String>,
    pub email_received_at: Option<DateTime<Utc>>,
}

/// Newest first. `None` when the ticket is not visible to the actor.
pub async fn list(
    pool: &Pool,
    actor: Id,
    ticket_id: Id,
    page: Page,
) -> Result<Option<(Vec<Message>, i64)>> {
    let mut conn = pool.acquire().await?;
    if tickets::visible_project(&mut conn, actor, ticket_id)
        .await?
        .is_none()
    {
        return Ok(None);
    }
    let total = sqlx::query_scalar("SELECT count(*) FROM messages WHERE ticket_id = $1")
        .bind(ticket_id)
        .fetch_one(&mut *conn)
        .await?;
    let rows = sqlx::query_as(&format!(
        "{SELECT} WHERE m.ticket_id = $1 ORDER BY m.created_at DESC, m.id DESC LIMIT $2 OFFSET $3"
    ))
    .bind(ticket_id)
    .bind(page.limit)
    .bind(page.offset)
    .fetch_all(&mut *conn)
    .await?;
    Ok(Some((rows, total)))
}

pub async fn get(pool: &Pool, actor: Id, id: Id) -> Result<Option<Message>> {
    sqlx::query_as(&format!(
        "{SELECT} JOIN tickets t ON t.id = m.ticket_id {} WHERE m.id = $2",
        tickets::VISIBLE
    ))
    .bind(actor)
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// An agent reply or internal note. Bumps the ticket, stamps `first_response_at` on the first
/// public reply, and links `attachment_ids`; any failure rolls everything back.
pub async fn post_agent(
    pool: &Pool,
    actor: Id,
    ticket_id: Id,
    new: &NewAgentMessage,
) -> Result<Posted> {
    let mut tx = pool.begin().await?;
    let Some(project_id) = tickets::visible_project(&mut tx, actor, ticket_id).await? else {
        return Ok(Posted::NoAccess);
    };
    let id = new_id();
    let kind = if new.is_internal {
        MessageType::Note
    } else {
        MessageType::Reply
    };
    sqlx::query(
        "INSERT INTO messages (id, project_id, ticket_id, sender_user_id, sender_type, sender_email,
                               sender_name, body, message_type, is_internal, email_message_id,
                               email_in_reply_to, email_references, email_to, email_subject)
         SELECT $1, $2, $3, u.id, 'agent', u.email, u.name, $5, $6, $7, $8, $9, $10, $11, $12
         FROM users u WHERE u.id = $4",
    )
    .bind(id)
    .bind(project_id)
    .bind(ticket_id)
    .bind(actor)
    .bind(&new.body)
    .bind(kind)
    .bind(new.is_internal)
    .bind(&new.email_message_id)
    .bind(&new.email_in_reply_to)
    .bind(&new.email_references)
    .bind(&new.email_to)
    .bind(&new.email_subject)
    .execute(&mut *tx)
    .await?;
    let mut ids = new.attachment_ids.clone();
    ids.sort();
    ids.dedup();
    if !ids.is_empty() {
        let linked = sqlx::query(
            "UPDATE attachments SET message_id = $1
             WHERE id = ANY($2) AND ticket_id = $3 AND message_id IS NULL",
        )
        .bind(id)
        .bind(&ids)
        .bind(ticket_id)
        .execute(&mut *tx)
        .await?;
        if linked.rows_affected() != ids.len() as u64 {
            return Ok(Posted::BadAttachments);
        }
    }
    sqlx::query(
        "UPDATE tickets SET updated_at = now(),
                first_response_at = COALESCE(first_response_at, CASE WHEN $2 THEN NULL ELSE now() END)
         WHERE id = $1",
    )
    .bind(ticket_id)
    .bind(new.is_internal)
    .execute(&mut *tx)
    .await?;
    if let (Some(out), false) = (&new.outbox, new.is_internal) {
        let log_id = new_id();
        sqlx::query(
            "INSERT INTO email_logs (id, project_id, ticket_id, message_id, to_email, subject, sent_by_user_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(log_id)
        .bind(project_id)
        .bind(ticket_id)
        .bind(id)
        .bind(&out.to_email)
        .bind(&out.subject)
        .bind(actor)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO mail_outbox (id, project_id, ticket_id, email_log_id, kind, message_id, sent_by_user_id)
             VALUES ($1, $2, $3, $4, 'reply', $5, $6)",
        )
        .bind(new_id())
        .bind(project_id)
        .bind(ticket_id)
        .bind(log_id)
        .bind(id)
        .bind(actor)
        .execute(&mut *tx)
        .await?;
    }
    let message = sqlx::query_as(&format!("{SELECT} WHERE m.id = $1"))
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Posted::Ok(message))
}

/// System side: a client's email on an existing ticket. Returns `None` when the Message-ID was
/// already stored for the project (replay) or the ticket is not in `project_id`.
/// Reopens resolved/closed tickets to `in_progress` and marks the ticket unread.
pub async fn post_inbound(pool: &Pool, new: &NewInbound) -> Result<Option<Message>> {
    let mut tx = pool.begin().await?;
    let locked: Option<Id> =
        sqlx::query_scalar("SELECT id FROM tickets WHERE id = $1 AND project_id = $2 FOR UPDATE")
            .bind(new.ticket_id)
            .bind(new.project_id)
            .fetch_optional(&mut *tx)
            .await?;
    if locked.is_none() {
        return Ok(None);
    }
    let id = new_id();
    let inserted = sqlx::query(
        "INSERT INTO messages (id, project_id, ticket_id, sender_type, sender_email, sender_name, body,
                               html_body, email_message_id, email_in_reply_to, email_references,
                               email_from, email_to, email_subject, email_received_at)
         VALUES ($1, $2, $3, 'client', $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
         ON CONFLICT (project_id, email_message_id) WHERE email_message_id IS NOT NULL DO NOTHING",
    )
    .bind(id)
    .bind(new.project_id)
    .bind(new.ticket_id)
    .bind(&new.sender_email)
    .bind(&new.sender_name)
    .bind(&new.body)
    .bind(&new.html_body)
    .bind(&new.email_message_id)
    .bind(&new.email_in_reply_to)
    .bind(&new.email_references)
    .bind(&new.email_from)
    .bind(&new.email_to)
    .bind(&new.email_subject)
    .bind(new.email_received_at)
    .execute(&mut *tx)
    .await?;
    if inserted.rows_affected() == 0 {
        return Ok(None);
    }
    sqlx::query(
        "UPDATE tickets SET updated_at = now(),
                status = CASE WHEN status IN ('resolved', 'closed') THEN 'in_progress' ELSE status END,
                closed_at = CASE WHEN status IN ('resolved', 'closed') THEN NULL ELSE closed_at END
         WHERE id = $1",
    )
    .bind(new.ticket_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO unread_tickets (id, project_id, ticket_id) VALUES ($1, $2, $3)
         ON CONFLICT (ticket_id) DO NOTHING",
    )
    .bind(new_id())
    .bind(new.project_id)
    .bind(new.ticket_id)
    .execute(&mut *tx)
    .await?;
    let message = sqlx::query_as(&format!("{SELECT} WHERE m.id = $1"))
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(message))
}

/// Append a line to a stored message (e.g. a note that an attachment could not be kept).
pub async fn append_body(pool: &Pool, project_id: Id, message_id: Id, text: &str) -> Result<()> {
    sqlx::query(
        "UPDATE messages SET body = body || E'\\n\\n' || $3, updated_at = now()
         WHERE id = $2 AND project_id = $1",
    )
    .bind(project_id)
    .bind(message_id)
    .bind(text)
    .execute(pool)
    .await?;
    Ok(())
}

/// Threading: the ticket (within the project) owning any of the given Message-IDs.
pub async fn find_ticket_by_message_ids(
    pool: &Pool,
    project_id: Id,
    message_ids: &[String],
) -> Result<Option<Id>> {
    sqlx::query_scalar(
        "SELECT ticket_id FROM messages WHERE project_id = $1 AND email_message_id = ANY($2)
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(project_id)
    .bind(message_ids)
    .fetch_optional(pool)
    .await
}

pub async fn message_id_exists(
    pool: &Pool,
    project_id: Id,
    email_message_id: &str,
) -> Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM messages WHERE project_id = $1 AND email_message_id = $2)",
    )
    .bind(project_id)
    .bind(email_message_id)
    .fetch_one(pool)
    .await
}

/// Store the mail headers of an agent reply after it was sent, so the client's answer threads to it.
pub async fn set_email_headers(
    pool: &Pool,
    message_id: Id,
    email_message_id: &str,
    in_reply_to: Option<&str>,
    references: Option<&str>,
    to: &str,
    subject: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE messages SET email_message_id = $2, email_in_reply_to = $3, email_references = $4, \
         email_to = $5, email_subject = $6 WHERE id = $1 AND email_message_id IS NULL",
    )
    .bind(message_id)
    .bind(email_message_id)
    .bind(in_reply_to)
    .bind(references)
    .bind(to)
    .bind(subject)
    .execute(pool)
    .await?;
    Ok(())
}
