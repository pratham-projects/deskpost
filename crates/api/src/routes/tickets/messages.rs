//! Agent replies, internal notes and the thread of a ticket.

use std::time::Duration;

use axum::extract::State;
use sd_db::messages::{self, NewAgentMessage, Posted};
use sd_db::tickets;
use sd_db::{Id, email_logs};
use serde::Deserialize;

use super::Params;
use super::dto::MessageDto;
use crate::error::{ApiError, Envelope, Fields, Pagination};
use crate::extract::{ApiJson, ApiPath, AuthUser};
use crate::mail_out::{MailError, OutboundMailer, ReplyMail};
use crate::state::AppState;

const MAX_BODY: usize = 50_000;
const MAX_ATTACHMENTS: usize = 20;
const MAIL_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_LOGGED_ERROR: usize = 500;

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
    let new = NewAgentMessage {
        body: req.body,
        is_internal: req.is_internal,
        attachment_ids: req.attachment_ids,
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
    // Internal notes are never emailed; the reply is already committed, so nothing below may fail the request.
    if !message.is_internal && req.send_email != Some(false) {
        deliver(&s, user.id, &message).await;
    }
    Ok(Envelope::created(MessageDto::new(
        message,
        &s.config.api_version,
    )))
}

/// Hand the saved reply to the mailer. Delivery failures are recorded in `email_logs` by the mailer;
/// other failures are recorded here. Nothing here fails the request.
async fn deliver(s: &AppState, actor: Id, message: &messages::Message) {
    let ticket = match tickets::get(&s.pool, actor, message.ticket_id).await {
        Ok(Some(t)) => t,
        Ok(None) => return,
        Err(e) => {
            tracing::error!(error = %e, "reply saved but ticket lookup for mail failed");
            return;
        }
    };
    let mail = ReplyMail {
        project_id: ticket.project_id,
        ticket_id: ticket.id,
        sent_by: Some(actor),
        body: message.body.clone(),
        attachment_ids: message.attachment_ids.clone(),
    };
    let error = match tokio::time::timeout(MAIL_TIMEOUT, s.mailer.send_reply(mail)).await {
        Ok(Ok(sent)) => {
            if let Err(e) = messages::set_email_headers(
                &s.pool,
                message.id,
                &sent.email_message_id,
                sent.in_reply_to.as_deref(),
                sent.references.as_deref(),
                &sent.to,
                &sent.subject,
            )
            .await
            {
                tracing::error!(error = %e, "reply sent but mail headers not stored");
            }
            return;
        }
        Ok(Err(MailError::Delivery(e))) => {
            tracing::warn!(ticket_id = %ticket.id, error = %e, "reply email failed");
            return;
        }
        Ok(Err(MailError::NotConfigured)) => "SMTP not configured".to_string(),
        Ok(Err(MailError::Internal(e))) => e,
        Err(_) => "mailer timed out".to_string(),
    };
    tracing::warn!(ticket_id = %ticket.id, %error, "reply email failed");
    let error: String = error.chars().take(MAX_LOGGED_ERROR).collect();
    let log = email_logs::NewEmailLog {
        project_id: ticket.project_id,
        ticket_id: ticket.id,
        message_id: Some(message.id),
        to_email: ticket.client_email,
        subject: Some(ticket.subject),
        sent_by_user_id: Some(actor),
    };
    let recorded = async {
        let row = email_logs::create(&s.pool, &log).await?;
        email_logs::mark_failed(&s.pool, row.id, &error).await
    };
    if let Err(e) = recorded.await {
        tracing::error!(error = %e, "could not record failed reply email");
    }
}
