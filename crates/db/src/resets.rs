//! Password-reset tokens (`password_resets`). Only the token's hash is stored.

use chrono::{DateTime, Utc};

use crate::{Id, Pool, Result, Status, new_id};

/// Store a fresh token for `user_id`, replacing any earlier one so only the newest link works.
/// Also drops expired and spent rows (housekeeping rides on the write path).
pub async fn create(
    pool: &Pool,
    user_id: Id,
    token_hash: &str,
    expires_at: DateTime<Utc>,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM password_resets WHERE user_id = $1 OR expires_at < now() OR used_at IS NOT NULL")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO password_resets (id, user_id, token_hash, expires_at) VALUES ($1, $2, $3, $4)",
    )
    .bind(new_id())
    .bind(user_id)
    .bind(token_hash)
    .bind(expires_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// Spend the token and set the new password in one transaction: the token is valid once, only
/// before `expires_at`, and only for an active account. Every refresh family of the user is
/// revoked and all other reset tokens are dropped. Returns the user's email, or `None` when the
/// token is unknown, expired, already used or the account is not active.
pub async fn consume(pool: &Pool, token_hash: &str, password_hash: &str) -> Result<Option<String>> {
    let mut tx = pool.begin().await?;
    let spent: Option<(Id,)> = sqlx::query_as(
        "UPDATE password_resets r SET used_at = now()
         FROM users u
         WHERE r.token_hash = $1 AND r.used_at IS NULL AND r.expires_at > now()
           AND u.id = r.user_id AND u.status = $2
         RETURNING r.user_id",
    )
    .bind(token_hash)
    .bind(Status::Active)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((user_id,)) = spent else {
        return Ok(None);
    };
    let (email,): (String,) = sqlx::query_as(
        "UPDATE users SET password_hash = $2, updated_at = now() WHERE id = $1 RETURNING email",
    )
    .bind(user_id)
    .bind(password_hash)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE refresh_families SET revoked_at = now(), revoked_reason = 'password_reset'
         WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM password_resets WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(email))
}
