//! Refresh-token families. A sign-in starts a family; each refresh rotates the token inside it.
//! Presenting an already-rotated token after the grace window revokes the family (reuse detection).

use chrono::{DateTime, Duration, Utc};
use sqlx::FromRow;

use crate::{Id, Pool, Result, new_id};

#[derive(Debug, Clone, Default)]
pub struct ClientMeta {
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    /// Old token consumed, new one stored.
    Rotated { user_id: Id, family_id: Id },
    /// A parallel request just rotated this token; answer 401 but keep the family alive.
    Grace,
    /// A rotated token came back after the grace window: the family is now revoked.
    Reused,
    /// Unknown, expired or already revoked.
    Invalid,
}

#[derive(Debug, Clone, FromRow)]
pub struct Family {
    pub id: Id,
    pub user_id: Id,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revoked_reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Sign-in: new family plus its first token. Returns the family id.
pub async fn start_family(
    pool: &Pool,
    user_id: Id,
    token_hash: &str,
    token_expires_at: DateTime<Utc>,
    family_expires_at: DateTime<Utc>,
    meta: &ClientMeta,
) -> Result<Id> {
    let family_id = new_id();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO refresh_families (id, user_id, expires_at) VALUES ($1, $2, $3)")
        .bind(family_id)
        .bind(user_id)
        .bind(family_expires_at)
        .execute(&mut *tx)
        .await?;
    insert_token(
        &mut tx,
        new_id(),
        family_id,
        token_hash,
        token_expires_at.min(family_expires_at),
        meta,
    )
    .await?;
    tx.commit().await?;
    Ok(family_id)
}

async fn insert_token(
    tx: &mut sqlx::PgConnection,
    id: Id,
    family_id: Id,
    token_hash: &str,
    expires_at: DateTime<Utc>,
    meta: &ClientMeta,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO refresh_tokens (id, family_id, token_hash, expires_at, ip_address, user_agent)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(family_id)
    .bind(token_hash)
    .bind(expires_at)
    .bind(&meta.ip_address)
    .bind(&meta.user_agent)
    .execute(tx)
    .await?;
    Ok(())
}

#[derive(FromRow)]
struct Presented {
    id: Id,
    family_id: Id,
    user_id: Id,
    expires_at: DateTime<Utc>,
    rotated_at: Option<DateTime<Utc>>,
    family_expires_at: DateTime<Utc>,
    family_revoked_at: Option<DateTime<Utc>>,
}

/// Exchange `presented_hash` for `new_hash`. `new_expires_at` is capped by the family's absolute expiry.
pub async fn rotate(
    pool: &Pool,
    presented_hash: &str,
    new_hash: &str,
    new_expires_at: DateTime<Utc>,
    grace: Duration,
    meta: &ClientMeta,
) -> Result<Rotation> {
    let mut tx = pool.begin().await?;
    let row: Option<Presented> = sqlx::query_as(
        "SELECT t.id, t.family_id, f.user_id, t.expires_at, t.rotated_at,
                f.expires_at AS family_expires_at, f.revoked_at AS family_revoked_at
         FROM refresh_tokens t JOIN refresh_families f ON f.id = t.family_id
         WHERE t.token_hash = $1 FOR UPDATE OF t",
    )
    .bind(presented_hash)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(p) = row else {
        return Ok(Rotation::Invalid);
    };
    let now = Utc::now();
    if p.family_revoked_at.is_some() || p.expires_at <= now || p.family_expires_at <= now {
        return Ok(Rotation::Invalid);
    }
    if let Some(rotated_at) = p.rotated_at {
        if now - rotated_at <= grace {
            return Ok(Rotation::Grace);
        }
        revoke_family_tx(&mut tx, p.family_id, "reuse_detected").await?;
        tx.commit().await?;
        return Ok(Rotation::Reused);
    }
    let next_id = new_id();
    insert_token(
        &mut tx,
        next_id,
        p.family_id,
        new_hash,
        new_expires_at.min(p.family_expires_at),
        meta,
    )
    .await?;
    sqlx::query("UPDATE refresh_tokens SET rotated_at = now(), replaced_by = $2 WHERE id = $1")
        .bind(p.id)
        .bind(next_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Rotation::Rotated {
        user_id: p.user_id,
        family_id: p.family_id,
    })
}

async fn revoke_family_tx(tx: &mut sqlx::PgConnection, family_id: Id, reason: &str) -> Result<u64> {
    let r = sqlx::query(
        "UPDATE refresh_families SET revoked_at = now(), revoked_reason = $2
         WHERE id = $1 AND revoked_at IS NULL",
    )
    .bind(family_id)
    .bind(reason)
    .execute(tx)
    .await?;
    Ok(r.rows_affected())
}

pub async fn revoke_family(pool: &Pool, family_id: Id, reason: &str) -> Result<bool> {
    let mut conn = pool.acquire().await?;
    Ok(revoke_family_tx(&mut conn, family_id, reason).await? == 1)
}

/// Sign-out: revoke the family a presented token belongs to. Unknown tokens are ignored.
pub async fn revoke_by_token(pool: &Pool, token_hash: &str) -> Result<bool> {
    let r = sqlx::query(
        "UPDATE refresh_families SET revoked_at = now(), revoked_reason = 'logout'
         WHERE revoked_at IS NULL
           AND id = (SELECT family_id FROM refresh_tokens WHERE token_hash = $1)",
    )
    .bind(token_hash)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() == 1)
}

/// Sign-out everywhere. Returns the number of families revoked.
pub async fn revoke_all_for_user(pool: &Pool, user_id: Id, reason: &str) -> Result<u64> {
    let r = sqlx::query(
        "UPDATE refresh_families SET revoked_at = now(), revoked_reason = $2
         WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .bind(reason)
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}

pub async fn get_family(pool: &Pool, family_id: Id) -> Result<Option<Family>> {
    sqlx::query_as(
        "SELECT id, user_id, expires_at, revoked_at, revoked_reason, created_at
         FROM refresh_families WHERE id = $1",
    )
    .bind(family_id)
    .fetch_optional(pool)
    .await
}

/// Housekeeping: drop families past their absolute expiry (tokens cascade).
pub async fn purge_expired(pool: &Pool) -> Result<u64> {
    let r = sqlx::query("DELETE FROM refresh_families WHERE expires_at < now()")
        .execute(pool)
        .await?;
    Ok(r.rows_affected())
}
