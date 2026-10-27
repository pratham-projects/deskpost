use chrono::{DateTime, Utc};
use sqlx::FromRow;

use crate::{Id, Page, Pool, Priority, Result, TicketSource, TicketStatus, clients, new_id};

#[derive(Debug, Clone, FromRow)]
pub struct Ticket {
    pub id: Id,
    pub project_id: Id,
    pub ticket_number: i64,
    pub client_id: Id,
    pub subject: String,
    pub description: Option<String>,
    pub source: TicketSource,
    pub status: TicketStatus,
    pub priority: Priority,
    pub assigned_to_user_id: Option<Id>,
    pub created_by_user_id: Option<Id>,
    pub first_response_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub client_email: String,
    pub client_name: Option<String>,
    pub project_name: String,
    pub assignee_email: Option<String>,
    pub assignee_name: Option<String>,
}

const COLS: &str = "t.id, t.project_id, t.ticket_number, t.client_id, t.subject, t.description,
    t.source, t.status, t.priority, t.assigned_to_user_id, t.created_by_user_id, t.first_response_at,
    t.closed_at, t.created_at, t.updated_at, c.email AS client_email, c.name AS client_name,
    p.name AS project_name, au.email AS assignee_email, au.name AS assignee_name";

const FROM: &str = "FROM tickets t JOIN clients c ON c.id = t.client_id
    JOIN projects p ON p.id = t.project_id LEFT JOIN users au ON au.id = t.assigned_to_user_id";

/// Tickets visible to `$1`: member of the project, and either allowed to see all tickets or
/// assigned to / creator of this one.
pub(crate) const VISIBLE: &str = "JOIN project_members pm ON pm.project_id = t.project_id
    AND pm.user_id = $1
    AND (pm.can_view_all_tickets OR t.assigned_to_user_id = pm.user_id OR t.created_by_user_id = pm.user_id)";

/// Project id of a ticket the actor may see, `None` otherwise.
pub(crate) async fn visible_project(
    conn: &mut sqlx::PgConnection,
    actor: Id,
    ticket_id: Id,
) -> Result<Option<Id>> {
    sqlx::query_scalar(&format!(
        "SELECT t.project_id FROM tickets t {VISIBLE} WHERE t.id = $2"
    ))
    .bind(actor)
    .bind(ticket_id)
    .fetch_optional(conn)
    .await
}

/// Who is creating a ticket; the check happens inside the create transaction.
#[derive(Debug, Clone)]
pub enum Access {
    /// Signed-in agent (`POST /tickets/admin`): must be a member.
    Member(Id),
    /// Public widget: the project API key hash must match this project and the project be active.
    ApiKeyHash(String),
    /// Inbound mail / workers; the caller already routed to the project.
    System,
}

#[derive(Debug, Clone)]
pub struct NewTicket {
    pub project_id: Id,
    pub client_email: String,
    pub client_name: Option<String>,
    pub subject: String,
    pub description: Option<String>,
    pub priority: Priority,
    pub source: TicketSource,
    pub created_by: Option<Id>,
    /// Body of the first (client) message.
    pub first_message: String,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TicketStats {
    pub pending: i64,
    pub in_progress: i64,
    pub on_hold: i64,
    pub resolved: i64,
    pub closed: i64,
    pub all: i64,
}

impl TicketStats {
    fn add(&mut self, status: TicketStatus, n: i64) {
        match status {
            TicketStatus::Pending => self.pending += n,
            TicketStatus::InProgress => self.in_progress += n,
            TicketStatus::OnHold => self.on_hold += n,
            TicketStatus::Resolved => self.resolved += n,
            TicketStatus::Closed => self.closed += n,
        }
        self.all += n;
    }
}

#[derive(Debug, Default, Clone)]
pub struct TicketFilter {
    pub project_id: Option<Id>,
    pub status: Option<TicketStatus>,
    pub assigned_to: Option<Id>,
    pub priority: Option<Priority>,
    pub unread_first: bool,
}

#[derive(Debug, Default, Clone)]
pub struct TicketPatch {
    pub subject: Option<String>,
    pub description: Option<String>,
    pub status: Option<TicketStatus>,
    pub priority: Option<Priority>,
    /// `Some(None)` unassigns, `Some(Some(id))` assigns (must be a member: FK error otherwise).
    pub assignee: Option<Option<Id>>,
}

#[derive(Debug, Clone)]
pub struct TicketUpdate {
    pub ticket: Ticket,
    pub previous_status: TicketStatus,
}

/// Creates client (get-or-create within the project), the next per-project ticket number, the
/// ticket, its first client message and the unread marker in one transaction.
/// `None` when the access check fails.
pub async fn create(pool: &Pool, access: &Access, new: &NewTicket) -> Result<Option<Ticket>> {
    let mut tx = pool.begin().await?;
    let allowed: bool = match access {
        Access::Member(user) => {
            sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM project_members WHERE project_id = $1 AND user_id = $2)",
            )
            .bind(new.project_id)
            .bind(user)
            .fetch_one(&mut *tx)
            .await?
        }
        Access::ApiKeyHash(hash) => {
            sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM projects WHERE id = $1 AND api_key_hash = $2 AND status = 'active')",
            )
            .bind(new.project_id)
            .bind(hash)
            .fetch_one(&mut *tx)
            .await?
        }
        Access::System => {
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM projects WHERE id = $1)")
                .bind(new.project_id)
                .fetch_one(&mut *tx)
                .await?
        }
    };
    if !allowed {
        return Ok(None);
    }
    let client = clients::get_or_create(
        &mut tx,
        new.project_id,
        &new.client_email,
        new.client_name.as_deref(),
    )
    .await?;
    // The row lock on the project serialises allocation, so numbers are 1..n without gaps or repeats.
    let number: i64 = sqlx::query_scalar(
        "UPDATE projects SET next_ticket_number = next_ticket_number + 1 WHERE id = $1
         RETURNING next_ticket_number - 1",
    )
    .bind(new.project_id)
    .fetch_one(&mut *tx)
    .await?;
    let id = new_id();
    sqlx::query(
        "INSERT INTO tickets (id, project_id, ticket_number, client_id, created_by_user_id, subject,
                              description, source, priority)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(new.project_id)
    .bind(number)
    .bind(client.id)
    .bind(new.created_by)
    .bind(&new.subject)
    .bind(&new.description)
    .bind(new.source)
    .bind(new.priority)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO messages (id, project_id, ticket_id, sender_type, sender_email, sender_name, body)
         VALUES ($1, $2, $3, 'client', $4, $5, $6)",
    )
    .bind(new_id())
    .bind(new.project_id)
    .bind(id)
    .bind(&client.email)
    .bind(&client.name)
    .bind(&new.first_message)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO unread_tickets (id, project_id, ticket_id) VALUES ($1, $2, $3)")
        .bind(new_id())
        .bind(new.project_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let ticket = sqlx::query_as(&format!("SELECT {COLS} {FROM} WHERE t.id = $1"))
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(ticket))
}

pub async fn get(pool: &Pool, actor: Id, id: Id) -> Result<Option<Ticket>> {
    sqlx::query_as(&format!("SELECT {COLS} {FROM} {VISIBLE} WHERE t.id = $2"))
        .bind(actor)
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// System side: ticket of a project by its number (inbound mail threading).
pub async fn find_by_number(pool: &Pool, project_id: Id, number: i64) -> Result<Option<Id>> {
    sqlx::query_scalar("SELECT id FROM tickets WHERE project_id = $1 AND ticket_number = $2")
        .bind(project_id)
        .bind(number)
        .fetch_optional(pool)
        .await
}

/// Newest-updated first (`unread_first` floats unread tickets to the top).
pub async fn list(
    pool: &Pool,
    actor: Id,
    filter: &TicketFilter,
    page: Page,
) -> Result<(Vec<Ticket>, i64)> {
    let cond = "WHERE ($2::uuid IS NULL OR t.project_id = $2) AND ($3::text IS NULL OR t.status = $3)
        AND ($4::uuid IS NULL OR t.assigned_to_user_id = $4) AND ($5::text IS NULL OR t.priority = $5)";
    let total = sqlx::query_scalar(&format!("SELECT count(*) FROM tickets t {VISIBLE} {cond}"))
        .bind(actor)
        .bind(filter.project_id)
        .bind(filter.status)
        .bind(filter.assigned_to)
        .bind(filter.priority)
        .fetch_one(pool)
        .await?;
    let rows = sqlx::query_as(&format!(
        "SELECT {COLS} {FROM} {VISIBLE} {cond}
         ORDER BY ($6 AND EXISTS (SELECT 1 FROM unread_tickets u WHERE u.ticket_id = t.id)) DESC,
                  t.updated_at DESC, t.id DESC
         LIMIT $7 OFFSET $8"
    ))
    .bind(actor)
    .bind(filter.project_id)
    .bind(filter.status)
    .bind(filter.assigned_to)
    .bind(filter.priority)
    .bind(filter.unread_first)
    .bind(page.limit)
    .bind(page.offset)
    .fetch_all(pool)
    .await?;
    Ok((rows, total))
}

/// Case-insensitive substring search over subject, description and message bodies.
/// Queries shorter than 2 characters match nothing.
pub async fn search(
    pool: &Pool,
    actor: Id,
    q: &str,
    project_id: Option<Id>,
    page: Page,
) -> Result<(Vec<Ticket>, i64)> {
    let q = q.trim();
    if q.chars().count() < 2 {
        return Ok((Vec::new(), 0));
    }
    let pattern = crate::like_pattern(q);
    let cond = "WHERE ($3::uuid IS NULL OR t.project_id = $3)
        AND (t.subject ILIKE $2 OR t.description ILIKE $2
             OR EXISTS (SELECT 1 FROM messages m WHERE m.ticket_id = t.id AND m.body ILIKE $2))";
    let total = sqlx::query_scalar(&format!("SELECT count(*) FROM tickets t {VISIBLE} {cond}"))
        .bind(actor)
        .bind(&pattern)
        .bind(project_id)
        .fetch_one(pool)
        .await?;
    let rows = sqlx::query_as(&format!(
        "SELECT {COLS} {FROM} {VISIBLE} {cond} ORDER BY t.updated_at DESC, t.id DESC LIMIT $4 OFFSET $5"
    ))
    .bind(actor)
    .bind(&pattern)
    .bind(project_id)
    .bind(page.limit)
    .bind(page.offset)
    .fetch_all(pool)
    .await?;
    Ok((rows, total))
}

/// Status counts over the actor's visible tickets, optionally for one project.
pub async fn stats(pool: &Pool, actor: Id, project_id: Option<Id>) -> Result<TicketStats> {
    let rows: Vec<(TicketStatus, i64)> = sqlx::query_as(&format!(
        "SELECT t.status, count(*) FROM tickets t {VISIBLE}
         WHERE ($2::uuid IS NULL OR t.project_id = $2) GROUP BY t.status"
    ))
    .bind(actor)
    .bind(project_id)
    .fetch_all(pool)
    .await?;
    let mut s = TicketStats::default();
    for (status, n) in rows {
        s.add(status, n);
    }
    Ok(s)
}

/// `GET /tickets/counts`: global stats plus one entry per project that has visible tickets.
pub async fn counts(pool: &Pool, actor: Id) -> Result<(TicketStats, Vec<(Id, TicketStats)>)> {
    let rows: Vec<(Id, TicketStatus, i64)> = sqlx::query_as(&format!(
        "SELECT t.project_id, t.status, count(*) FROM tickets t {VISIBLE}
         GROUP BY t.project_id, t.status ORDER BY t.project_id"
    ))
    .bind(actor)
    .fetch_all(pool)
    .await?;
    let mut global = TicketStats::default();
    let mut per_project: Vec<(Id, TicketStats)> = Vec::new();
    for (project, status, n) in rows {
        global.add(status, n);
        match per_project.last_mut() {
            Some((p, s)) if *p == project => s.add(status, n),
            _ => {
                let mut s = TicketStats::default();
                s.add(status, n);
                per_project.push((project, s));
            }
        }
    }
    Ok((global, per_project))
}

/// Any visible ticket. Entering resolved/closed stamps `closed_at` once; leaving them clears it.
/// An assignee that is not a member of the project fails with a foreign-key violation.
pub async fn update(
    pool: &Pool,
    actor: Id,
    id: Id,
    patch: &TicketPatch,
) -> Result<Option<TicketUpdate>> {
    let mut tx = pool.begin().await?;
    let previous: Option<TicketStatus> = sqlx::query_scalar(&format!(
        "SELECT t.status FROM tickets t {VISIBLE} WHERE t.id = $2 FOR UPDATE OF t"
    ))
    .bind(actor)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(previous_status) = previous else {
        return Ok(None);
    };
    let (set_assignee, assignee) = match patch.assignee {
        Some(a) => (true, a),
        None => (false, None),
    };
    sqlx::query(
        "UPDATE tickets SET subject = COALESCE($2, subject), description = COALESCE($3, description),
                status = COALESCE($4, status), priority = COALESCE($5, priority),
                assigned_to_user_id = CASE WHEN $6 THEN $7 ELSE assigned_to_user_id END,
                closed_at = CASE WHEN $4::text IN ('resolved', 'closed') THEN COALESCE(closed_at, now())
                                 WHEN $4::text IS NOT NULL THEN NULL ELSE closed_at END,
                updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(&patch.subject)
    .bind(&patch.description)
    .bind(patch.status)
    .bind(patch.priority)
    .bind(set_assignee)
    .bind(assignee)
    .execute(&mut *tx)
    .await?;
    let ticket = sqlx::query_as(&format!("SELECT {COLS} {FROM} WHERE t.id = $1"))
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(TicketUpdate {
        ticket,
        previous_status,
    }))
}

/// Project-admin only. Cascades messages, attachment rows, logs, unread and viewers.
pub async fn delete(pool: &Pool, actor: Id, id: Id) -> Result<bool> {
    let r = sqlx::query(
        "DELETE FROM tickets t WHERE t.id = $2 AND EXISTS (
            SELECT 1 FROM project_members m
            WHERE m.project_id = t.project_id AND m.user_id = $1 AND m.role = 'admin')",
    )
    .bind(actor)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() == 1)
}

/// Auto-close job: resolved tickets idle for the project's `auto_close_days` become closed.
/// Returns `(project_id, ticket_id)` of every ticket closed.
pub async fn auto_close_resolved(pool: &Pool) -> Result<Vec<(Id, Id)>> {
    sqlx::query_as(
        "UPDATE tickets t SET status = 'closed', closed_at = COALESCE(t.closed_at, now()), updated_at = now()
         FROM projects p
         WHERE p.id = t.project_id AND p.auto_close_days > 0 AND t.status = 'resolved'
           AND t.updated_at < now() - make_interval(days => p.auto_close_days)
         RETURNING t.project_id, t.id",
    )
    .fetch_all(pool)
    .await
}
