//! Agent replies, internal notes and the thread of a ticket.

use axum::extract::State;
use sd_db::Id;
use sd_db::messages::{self, NewAgentMessage, Posted};
use sd_db::tickets;
use serde::Deserialize;

use super::Params;
use super::dto::MessageDto;
use crate::error::{ApiError, Envelope, Fields, Pagination};
use crate::extract::{ApiJson, ApiPath, AuthUser};
use crate::mail_out::reply_outbox;
use crate::state::AppState;

const MAX_BODY: usize = 50_000;
const MAX_ATTACHMENTS: usize = 20;

pub async fn list(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(ticket_id): ApiPath<Id>,
    q: Params,
) -> Result<Envelope<Vec<MessageDto>>, ApiError> {
    let mut f = Fields::default();
    let page = q.page(&mut f);
    f.finish()?;
    let (rows, total) = messages::list(&s.pool, user.id, ticket_id, page)
        .await?
        .ok_or_else(|| ApiError::not_found("Ticket"))?;
    let version = &s.config.api_version;
    Ok(Envelope::ok(
        rows.into_iter()
            .map(|m| MessageDto::new(m, version))
            .collect(),
    )
    .paginated(Pagination::new(total, page.limit, page.offset)))
}

#[derive(Deserialize)]
pub struct CreateReq {
    #[serde(default)]
    body: String,
    #[serde(default, rename = "isInternal")]
    is_internal: bool,
    #[serde(default, rename = "sendEmail")]
    send_email: Option<bool>,
    #[serde(default, rename = "attachmentIds")]
    attachment_ids: Vec<Id>,
}

pub async fn create(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(ticket_id): ApiPath<Id>,
    ApiJson(req): ApiJson<CreateReq>,
) -> Result<Envelope<MessageDto>, ApiError> {
    let mut f = Fields::default();
    if req.body.trim().is_empty() || req.body.chars().count() > MAX_BODY {
        f.add("body", "Body must be 1 to 50000 characters.");
    }
    if req.attachment_ids.len() > MAX_ATTACHMENTS {
        f.add("attachmentIds", "At most 20 attachments.");
    }
    f.finish()?;
    // Internal notes are never emailed. The reply is queued in the same transaction as the message
    // and sent by the outbox worker, so nothing here depends on SMTP.
    let outbox = if !req.is_internal && req.send_email != Some(false) {
        reply_outbox_for(&s, user.id, ticket_id).await?
    } else {
        None
    };
    let new = NewAgentMessage {
        body: req.body,
        is_internal: req.is_internal,
        attachment_ids: req.attachment_ids,
        outbox,
        ..NewAgentMessage::default()
    };
    let message = match messages::post_agent(&s.pool, user.id, ticket_id, &new).await? {
        Posted::Ok(m) => m,
        Posted::NoAccess => return Err(ApiError::not_found("Ticket")),
        Posted::BadAttachments => {
            let mut f = Fields::default();
            f.add(
                "attachmentIds",
                "Attachments must belong to this ticket and be unused.",
            );
            return Err(f.finish().unwrap_err());
        }
    };
    Ok(Envelope::created(MessageDto::new(
        message,
        &s.config.api_version,
    )))
}

/// Where an emailed reply would go, for `NewAgentMessage::outbox`. `None` when the actor cannot see
/// the ticket (the post then answers 404).
pub async fn reply_outbox_for(
    s: &AppState,
    actor: Id,
    ticket_id: Id,
) -> Result<Option<messages::ReplyOutbox>, ApiError> {
    let Some(ticket) = tickets::get(&s.pool, actor, ticket_id).await? else {
        return Ok(None);
    };
    Ok(reply_outbox(&s.pool, ticket.project_id, ticket.id).await?)
}
