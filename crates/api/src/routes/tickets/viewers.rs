//! Ticket presence (`docs/API.md` section 11): heartbeat rows that expire after two minutes.

use std::time::Duration;

use axum::extract::State;
use sd_db::{Id, Pool, tickets, viewers};
use serde::Serialize;
use serde_json::{Value, json};

use crate::error::{ApiError, Envelope};
use crate::extract::{ApiPath, AuthUser};
use crate::state::AppState;

const VIEWER_TTL_SECS: i64 = 120;
const GC_EVERY: Duration = Duration::from_secs(60);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerDto {
    user_id: Id,
    name: String,
}

pub async fn list(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(ticket_id): ApiPath<Id>,
) -> Result<Envelope<Vec<ViewerDto>>, ApiError> {
    let rows = viewers::list(&s.pool, user.id, ticket_id, VIEWER_TTL_SECS)
        .await?
        .ok_or_else(|| ApiError::not_found("Ticket"))?;
    Ok(Envelope::ok(
        rows.into_iter()
            .map(|v| ViewerDto {
                user_id: v.user_id,
                name: v.name,
            })
            .collect(),
    ))
}

pub async fn heartbeat(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(ticket_id): ApiPath<Id>,
) -> Result<Envelope<Value>, ApiError> {
    if !viewers::touch(&s.pool, user.id, ticket_id).await? {
        return Err(ApiError::not_found("Ticket"));
    }
    Ok(Envelope::ok(json!({ "success": true })))
}

pub async fn leave(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(ticket_id): ApiPath<Id>,
) -> Result<Envelope<Value>, ApiError> {
    tickets::get(&s.pool, user.id, ticket_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Ticket"))?;
    viewers::remove(&s.pool, user.id, ticket_id).await?;
    Ok(Envelope::ok(json!({ "success": true })))
}

/// Background GC of expired heartbeats; listing already ignores them, this only bounds the table.
pub fn spawn_presence_gc(pool: Pool) -> tokio::task::JoinHandle<()> {
    spawn_gc(pool, GC_EVERY)
}

pub fn spawn_gc(pool: Pool, every: Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        loop {
            tick.tick().await;
            if let Err(e) = viewers::purge_stale(&pool, VIEWER_TTL_SECS).await {
                tracing::warn!(error = %e, "presence gc failed");
            }
        }
    })
}
