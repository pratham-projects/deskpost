//! Unread markers (`docs/API.md` section 10). Per ticket, shared by every member.

use axum::extract::State;
use sd_db::{Id, tickets, unread};
use serde::Serialize;
use serde_json::{Value, json};

use super::Params;
use crate::dto::ts;
use crate::error::{ApiError, Envelope, Fields};
use crate::extract::{ApiPath, AuthUser};
use crate::state::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnreadItem {
    id: Id,
    ticket_id: Id,
    project_id: Id,
    created_at: String,
}

pub async fn list(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    q: Params,
) -> Result<Envelope<Vec<UnreadItem>>, ApiError> {
    let mut f = Fields::default();
    let project_id = q.uuid("projectId", &mut f);
    f.finish()?;
    let rows = unread::list(&s.pool, user.id, project_id).await?;
    Ok(Envelope::ok(
        rows.into_iter()
            .map(|u| UnreadItem {
                id: u.id,
                ticket_id: u.ticket_id,
                project_id: u.project_id,
                created_at: ts(u.created_at),
            })
            .collect(),
    ))
}

pub async fn mark_read(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(ticket_id): ApiPath<Id>,
) -> Result<Envelope<Value>, ApiError> {
    visible(&s, user.id, ticket_id).await?;
    let deleted = unread::clear(&s.pool, user.id, ticket_id).await?;
    Ok(Envelope::ok(json!({ "deleted": deleted })))
}

/// `created` is false when the ticket was already unread.
pub async fn mark_unread(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(ticket_id): ApiPath<Id>,
) -> Result<Envelope<Value>, ApiError> {
    let project_id = visible(&s, user.id, ticket_id).await?;
    let created = unread::mark(&s.pool, project_id, ticket_id).await?;
    Ok(Envelope::ok(json!({ "created": created })))
}

async fn visible(s: &AppState, user: Id, ticket_id: Id) -> Result<Id, ApiError> {
    Ok(tickets::get(&s.pool, user, ticket_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Ticket"))?
        .project_id)
}
