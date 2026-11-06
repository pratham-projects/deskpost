//! sd-db: Postgres 16 schema, migrations and tenant-scoped queries.
//!
//! Conventions
//! - Functions that take an `actor` (user id) check project membership/role inside the SQL. A project
//!   the actor does not belong to is indistinguishable from a missing one: `None`/`false`/empty.
//!   Use [`members::role_of`] when the caller needs to tell "not found" (404) from "not allowed" (403).
//! - Functions without an `actor` are for the system side (inbound mail, workers, public widget); their
//!   callers have already authenticated by other means (API key, signature, process identity).
//! - The crate never sees plaintext mail passwords; it stores whatever ciphertext it is given.

use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

pub use sqlx::Error;
pub type Result<T> = sqlx::Result<T>;
pub type Pool = sqlx::PgPool;
pub type Id = Uuid;

pub mod attachments;
pub mod clients;
pub mod email_logs;
pub mod email_responses;
pub mod mailq;
pub mod members;
pub mod messages;
pub mod projects;
pub mod sessions;
pub mod templates;
pub mod tickets;
pub mod types;
pub mod unread;
pub mod users;
pub mod viewers;

pub use types::*;

/// Largest page any list function will return.
pub const MAX_LIMIT: i64 = 200;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Time-ordered UUID (v7) minted in Rust; works on Postgres 16.
pub fn new_id() -> Id {
    Uuid::now_v7()
}

pub async fn connect(database_url: &str, max_connections: u32) -> Result<Pool> {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(database_url)
        .await
}

pub async fn migrate(pool: &Pool) -> std::result::Result<(), sqlx::migrate::MigrateError> {
    MIGRATOR.run(pool).await
}

/// `limit`/`offset` as the API passes them; clamped so no query is unbounded.
#[derive(Debug, Clone, Copy)]
pub struct Page {
    pub limit: i64,
    pub offset: i64,
}

impl Page {
    pub fn new(limit: i64, offset: i64) -> Self {
        Self {
            limit: limit.clamp(1, MAX_LIMIT),
            offset: offset.max(0),
        }
    }
}

fn code(e: &Error) -> Option<String> {
    e.as_database_error()
        .and_then(|d| d.code().map(|c| c.into_owned()))
}

/// 23505: a UNIQUE constraint or index rejected the row (API: 409).
pub fn is_unique_violation(e: &Error) -> bool {
    code(e).as_deref() == Some("23505")
}

/// 23503: a foreign key rejected the row (unknown assignee, user still owns a project, ...).
pub fn is_foreign_key_violation(e: &Error) -> bool {
    code(e).as_deref() == Some("23503")
}

/// Name of the violated constraint/index, to tell e.g. a slug clash from a domain clash.
pub fn constraint(e: &Error) -> Option<&str> {
    e.as_database_error().and_then(|d| d.constraint())
}

/// Escape `%`, `_` and `\` so user text is matched literally by ILIKE.
pub(crate) fn like_pattern(q: &str) -> String {
    let mut out = String::with_capacity(q.len() + 2);
    out.push('%');
    for c in q.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_is_always_bounded() {
        assert_eq!(Page::new(0, -1).limit, 1);
        assert_eq!(Page::new(0, -1).offset, 0);
        assert_eq!(Page::new(1_000_000, 5).limit, MAX_LIMIT);
        assert_eq!(Page::new(25, 50).limit, 25);
    }

    #[test]
    fn like_patterns_match_literally() {
        assert_eq!(like_pattern("abc"), "%abc%");
        assert_eq!(like_pattern("50%_off\\"), "%50\\%\\_off\\\\%");
        assert_eq!(like_pattern(""), "%%");
    }

    #[test]
    fn enums_round_trip_through_their_text_form() {
        for s in ["pending", "in_progress", "on_hold", "resolved", "closed"] {
            assert_eq!(s.parse::<TicketStatus>().unwrap().as_str(), s);
        }
        for s in ["low", "normal", "high", "urgent"] {
            assert_eq!(s.parse::<Priority>().unwrap().as_str(), s);
        }
        assert_eq!("admin".parse::<Role>().unwrap(), Role::Admin);
        assert_eq!("suspended".parse::<Status>().unwrap(), Status::Suspended);
        assert!("Admin".parse::<Role>().is_err());
        assert!("done".parse::<TicketStatus>().is_err());
    }

    #[test]
    fn ids_are_time_ordered_v7() {
        let (a, b) = (new_id(), new_id());
        assert_eq!(a.get_version_num(), 7);
        assert!(a < b);
    }
}
