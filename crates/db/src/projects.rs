use chrono::{DateTime, Utc};
use sqlx::FromRow;

use crate::{Id, Page, Pool, Result, Status, new_id};

/// Public view of a project. Mail passwords are never selected, only whether they are set.
#[derive(Debug, Clone, FromRow)]
pub struct Project {
    pub id: Id,
    pub name: String,
    pub slug: String,
    pub domain: Option<String>,
    pub description: Option<String>,
    pub api_key_prefix: String,
    pub owner_user_id: Id,
    pub status: Status,
    pub is_archived: bool,
    pub reply_to_email: Option<String>,
    pub smtp_email: Option<String>,
    pub smtp_username: Option<String>,
    pub smtp_host: Option<String>,
    pub smtp_port: i32,
    pub has_smtp_password: bool,
    pub imap_host: Option<String>,
    pub imap_port: Option<i32>,
    pub imap_username: Option<String>,
    pub imap_folder: String,
    pub has_imap_password: bool,
    pub auto_close_days: i32,
    pub storage_quota_mb: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

const COLS: &str = "p.id, p.name, p.slug, p.domain, p.description, p.api_key_prefix, p.owner_user_id,
    p.status, p.is_archived, p.reply_to_email, p.smtp_email, p.smtp_username, p.smtp_host, p.smtp_port,
    p.smtp_password_encrypted IS NOT NULL AS has_smtp_password, p.imap_host, p.imap_port,
    p.imap_username, p.imap_folder, p.imap_password_encrypted IS NOT NULL AS has_imap_password,
    p.auto_close_days, p.storage_quota_mb, p.created_at, p.updated_at";

/// Mail settings including ciphertext, for workers and the project-admin settings screen.
#[derive(Clone, FromRow)]
pub struct MailSettings {
    pub project_id: Id,
    pub reply_to_email: Option<String>,
    pub email_from_name: String,
    pub enable_email_threading: bool,
    pub smtp_email: Option<String>,
    pub smtp_username: Option<String>,
    pub smtp_password_encrypted: Option<String>,
    pub smtp_host: Option<String>,
    pub smtp_port: i32,
    pub imap_host: Option<String>,
    pub imap_port: Option<i32>,
    pub imap_username: Option<String>,
    pub imap_password_encrypted: Option<String>,
    pub imap_folder: String,
    pub imap_uid_validity: Option<i64>,
    pub imap_last_uid: i64,
}

impl std::fmt::Debug for MailSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailSettings")
            .field("project_id", &self.project_id)
            .field("smtp_host", &self.smtp_host)
            .field("imap_host", &self.imap_host)
            .finish_non_exhaustive()
    }
}

const MAIL_COLS: &str = "id AS project_id, reply_to_email, email_from_name, enable_email_threading,
    smtp_email, smtp_username, smtp_password_encrypted, smtp_host, smtp_port, imap_host, imap_port,
    imap_username, imap_password_encrypted, imap_folder, imap_uid_validity, imap_last_uid";

#[derive(Debug, Clone)]
pub struct NewProject {
    pub name: String,
    pub slug: String,
    pub domain: Option<String>,
    pub description: Option<String>,
    pub api_key_hash: String,
    pub api_key_prefix: String,
}

#[derive(Debug, Default, Clone)]
pub struct ProjectPatch {
    pub name: Option<String>,
    pub slug: Option<String>,
    /// `Some("")` clears.
    pub domain: Option<String>,
    pub description: Option<String>,
    pub status: Option<Status>,
    pub is_archived: Option<bool>,
    pub auto_close_days: Option<i32>,
}

/// `None` leaves a column alone, `Some("")` clears it. Passwords are ciphertext made by the caller.
#[derive(Debug, Default, Clone)]
pub struct MailUpdate {
    pub reply_to_email: Option<String>,
    pub smtp_email: Option<String>,
    pub smtp_username: Option<String>,
    pub smtp_password_encrypted: Option<String>,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<i32>,
    pub imap_host: Option<String>,
    pub imap_port: Option<i32>,
    pub imap_username: Option<String>,
    pub imap_password_encrypted: Option<String>,
    pub imap_folder: Option<String>,
}

const ADMIN_OF: &str = "EXISTS (SELECT 1 FROM project_members m
    WHERE m.project_id = p.id AND m.user_id = $1 AND m.role = 'admin')";
const MEMBER_OF: &str = "EXISTS (SELECT 1 FROM project_members m
    WHERE m.project_id = p.id AND m.user_id = $1)";

/// Creates the project and makes `owner` its admin member with every permission.
pub async fn create(pool: &Pool, owner: Id, new: &NewProject) -> Result<Project> {
    let id = new_id();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO projects (id, name, slug, domain, description, api_key_hash, api_key_prefix, owner_user_id)
         VALUES ($1, $2, $3, NULLIF($4, ''), $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(&new.name)
    .bind(&new.slug)
    .bind(&new.domain)
    .bind(&new.description)
    .bind(&new.api_key_hash)
    .bind(&new.api_key_prefix)
    .bind(owner)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO project_members (id, project_id, user_id, role, should_notify, can_view_all_tickets, can_manage_templates)
         VALUES ($1, $2, $3, 'admin', true, true, true)",
    )
    .bind(new_id())
    .bind(id)
    .bind(owner)
    .execute(&mut *tx)
    .await?;
    let project = sqlx::query_as(&format!("SELECT {COLS} FROM projects p WHERE p.id = $1"))
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(project)
}

pub async fn list(
    pool: &Pool,
    actor: Id,
    status: Option<Status>,
    page: Page,
) -> Result<(Vec<Project>, i64)> {
    let cond = format!("WHERE {MEMBER_OF} AND ($2::text IS NULL OR p.status = $2)");
    let total = sqlx::query_scalar(&format!("SELECT count(*) FROM projects p {cond}"))
        .bind(actor)
        .bind(status)
        .fetch_one(pool)
        .await?;
    let rows = sqlx::query_as(&format!(
        "SELECT {COLS} FROM projects p {cond} ORDER BY p.created_at, p.id LIMIT $3 OFFSET $4"
    ))
    .bind(actor)
    .bind(status)
    .bind(page.limit)
    .bind(page.offset)
    .fetch_all(pool)
    .await?;
    Ok((rows, total))
}

pub async fn get(pool: &Pool, actor: Id, id: Id) -> Result<Option<Project>> {
    sqlx::query_as(&format!(
        "SELECT {COLS} FROM projects p WHERE p.id = $2 AND {MEMBER_OF}"
    ))
    .bind(actor)
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// Project-admin only; `None` when missing, foreign, or the actor is not an admin there.
pub async fn update(
    pool: &Pool,
    actor: Id,
    id: Id,
    patch: &ProjectPatch,
) -> Result<Option<Project>> {
    let mut tx = pool.begin().await?;
    if !apply_patch(&mut tx, actor, id, patch).await? {
        return Ok(None);
    }
    let project = reload(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Some(project))
}

/// [`update`] and [`set_mail_settings`] in one transaction, so a clash on one half (slug vs mailbox
/// address) leaves nothing half-applied.
pub async fn update_with_mail(
    pool: &Pool,
    actor: Id,
    id: Id,
    patch: &ProjectPatch,
    mail: &MailUpdate,
) -> Result<Option<Project>> {
    let mut tx = pool.begin().await?;
    if !apply_patch(&mut tx, actor, id, patch).await?
        || !apply_mail(&mut tx, actor, id, mail).await?
    {
        return Ok(None);
    }
    let project = reload(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Some(project))
}

async fn reload(conn: &mut sqlx::PgConnection, id: Id) -> Result<Project> {
    sqlx::query_as(&format!("SELECT {COLS} FROM projects p WHERE p.id = $1"))
        .bind(id)
        .fetch_one(conn)
        .await
}

async fn apply_patch(
    conn: &mut sqlx::PgConnection,
    actor: Id,
    id: Id,
    patch: &ProjectPatch,
) -> Result<bool> {
    let done = sqlx::query(&format!(
        "UPDATE projects p SET name = COALESCE($3, name), slug = COALESCE($4, slug),
                domain = NULLIF(COALESCE($5, domain), ''), description = COALESCE($6, description),
                status = COALESCE($7, status), is_archived = COALESCE($8, is_archived),
                auto_close_days = COALESCE($9, auto_close_days), updated_at = now()
         WHERE p.id = $2 AND {ADMIN_OF}"
    ))
    .bind(actor)
    .bind(id)
    .bind(&patch.name)
    .bind(&patch.slug)
    .bind(&patch.domain)
    .bind(&patch.description)
    .bind(patch.status)
    .bind(patch.is_archived)
    .bind(patch.auto_close_days)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Project-admin only. The old key stops matching immediately.
pub async fn replace_api_key(
    pool: &Pool,
    actor: Id,
    id: Id,
    api_key_hash: &str,
    api_key_prefix: &str,
) -> Result<Option<DateTime<Utc>>> {
    sqlx::query_scalar(&format!(
        "UPDATE projects p SET api_key_hash = $3, api_key_prefix = $4, updated_at = now()
         WHERE p.id = $2 AND {ADMIN_OF} RETURNING updated_at"
    ))
    .bind(actor)
    .bind(id)
    .bind(api_key_hash)
    .bind(api_key_prefix)
    .fetch_optional(pool)
    .await
}

/// Owner only. Cascades to tickets, messages, attachments rows, members, templates and responses.
pub async fn delete(pool: &Pool, actor: Id, id: Id) -> Result<bool> {
    let r = sqlx::query("DELETE FROM projects WHERE id = $2 AND owner_user_id = $1")
        .bind(actor)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() == 1)
}

/// Project-admin only: current mail settings (ciphertext included, for the settings screen to
/// report `has_*_password` and for re-saving).
pub async fn get_mail_settings(pool: &Pool, actor: Id, id: Id) -> Result<Option<MailSettings>> {
    sqlx::query_as(&format!(
        "SELECT {MAIL_COLS} FROM projects p WHERE p.id = $2 AND {ADMIN_OF}"
    ))
    .bind(actor)
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// Project-admin only.
pub async fn set_mail_settings(
    pool: &Pool,
    actor: Id,
    id: Id,
    u: &MailUpdate,
) -> Result<Option<Project>> {
    let mut tx = pool.begin().await?;
    if !apply_mail(&mut tx, actor, id, u).await? {
        return Ok(None);
    }
    let project = reload(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Some(project))
}

async fn apply_mail(
    conn: &mut sqlx::PgConnection,
    actor: Id,
    id: Id,
    u: &MailUpdate,
) -> Result<bool> {
    let done = sqlx::query(&format!(
        "UPDATE projects p SET
            reply_to_email = NULLIF(COALESCE($3, reply_to_email), ''),
            smtp_email = NULLIF(COALESCE($4, smtp_email), ''),
            smtp_username = NULLIF(COALESCE($5, smtp_username), ''),
            smtp_password_encrypted = NULLIF(COALESCE($6, smtp_password_encrypted), ''),
            smtp_host = NULLIF(COALESCE($7, smtp_host), ''),
            smtp_port = COALESCE($8, smtp_port),
            imap_host = NULLIF(COALESCE($9, imap_host), ''),
            imap_port = COALESCE($10, imap_port),
            imap_username = NULLIF(COALESCE($11, imap_username), ''),
            imap_password_encrypted = NULLIF(COALESCE($12, imap_password_encrypted), ''),
            imap_folder = COALESCE(NULLIF($13, ''), imap_folder),
            updated_at = now()
         WHERE p.id = $2 AND {ADMIN_OF}"
    ))
    .bind(actor)
    .bind(id)
    .bind(&u.reply_to_email)
    .bind(&u.smtp_email)
    .bind(&u.smtp_username)
    .bind(&u.smtp_password_encrypted)
    .bind(&u.smtp_host)
    .bind(u.smtp_port)
    .bind(&u.imap_host)
    .bind(u.imap_port)
    .bind(&u.imap_username)
    .bind(&u.imap_password_encrypted)
    .bind(&u.imap_folder)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

// ---- system side (no actor): public widget, inbound mail, workers ----

/// Public widget auth: the active, non-archived project whose API key hashes to `api_key_hash`.
pub async fn find_by_api_key_hash(pool: &Pool, api_key_hash: &str) -> Result<Option<Id>> {
    sqlx::query_scalar(
        "SELECT id FROM projects WHERE api_key_hash = $1 AND status = 'active' AND NOT is_archived",
    )
    .bind(api_key_hash)
    .fetch_optional(pool)
    .await
}

/// Inbound mail routing: the one active, non-archived project whose mailbox address is `email` and
/// an operator approved (`approve_mailbox`).
pub async fn find_by_smtp_email(pool: &Pool, email: &str) -> Result<Option<Id>> {
    sqlx::query_scalar("SELECT id FROM projects WHERE lower(smtp_email) = lower($1) AND status = 'active' AND NOT is_archived AND mail_verified_at IS NOT NULL")
        .bind(email)
        .fetch_optional(pool)
        .await
}

pub async fn mail_settings(pool: &Pool, id: Id) -> Result<Option<MailSettings>> {
    sqlx::query_as(&format!("SELECT {MAIL_COLS} FROM projects WHERE id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// Projects the IMAP poller should visit.
pub async fn list_imap_enabled(pool: &Pool) -> Result<Vec<MailSettings>> {
    sqlx::query_as(&format!(
        "SELECT {MAIL_COLS} FROM projects
         WHERE imap_host IS NOT NULL AND imap_username IS NOT NULL
           AND imap_password_encrypted IS NOT NULL AND status = 'active' AND NOT is_archived
         ORDER BY id"
    ))
    .fetch_all(pool)
    .await
}

/// Store the poll cursor. If the server's UIDVALIDITY changed, the caller passes `last_uid = 0`.
pub async fn set_imap_cursor(
    pool: &Pool,
    id: Id,
    uid_validity: i64,
    last_uid: i64,
) -> Result<bool> {
    let r =
        sqlx::query("UPDATE projects SET imap_uid_validity = $2, imap_last_uid = $3 WHERE id = $1")
            .bind(id)
            .bind(uid_validity)
            .bind(last_uid)
            .execute(pool)
            .await?;
    Ok(r.rows_affected() == 1)
}

/// Operator approval of the mailbox address the operator reviewed (`email`) for webhook routing.
/// `false` when the project no longer has that address. Cleared again by a trigger when the address or servers change.
pub async fn approve_mailbox(pool: &Pool, id: Id, email: &str) -> Result<bool> {
    let n = sqlx::query(
        "UPDATE projects SET mail_verified_at = now() WHERE id = $1 AND lower(smtp_email) = lower($2)",
    )
    .bind(id)
    .bind(email)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n == 1)
}

/// Whether the project's current address was approved.
pub async fn mail_is_verified(pool: &Pool, id: Id) -> Result<bool> {
    sqlx::query_scalar("SELECT mail_verified_at IS NOT NULL FROM projects WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
}
