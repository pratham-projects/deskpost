//! Wire shapes `Ticket`, `Message`, `UnreadItem` (`docs/API.md` section 2).

use sd_db::Id;
use sd_db::messages::Message;
use sd_db::tickets::{Ticket, TicketStats};
use serde::Serialize;
use serde_json::{Value, json};

use crate::dto::ts;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketDto {
    id: Id,
    ticket_number: i64,
    project_id: Id,
    client_id: Id,
    subject: String,
    description: Option<String>,
    status: &'static str,
    priority: &'static str,
    assigned_to_user_id: Option<Id>,
    created_at: String,
    updated_at: String,
    closed_at: Option<String>,
    client: ClientRef,
    project: ProjectRef,
    assigned_to: Option<UserRef>,
}

#[derive(Serialize)]
struct ClientRef {
    id: Id,
    email: String,
    name: Option<String>,
}

#[derive(Serialize)]
struct ProjectRef {
    id: Id,
    name: String,
}

#[derive(Serialize)]
struct UserRef {
    id: Id,
    email: String,
    name: String,
}

impl From<Ticket> for TicketDto {
    fn from(t: Ticket) -> Self {
        let assigned_to = match (t.assigned_to_user_id, t.assignee_email, t.assignee_name) {
            (Some(id), Some(email), Some(name)) => Some(UserRef { id, email, name }),
            _ => None,
        };
        Self {
            id: t.id,
            ticket_number: t.ticket_number,
            project_id: t.project_id,
            client_id: t.client_id,
            subject: t.subject,
            description: t.description,
            status: t.status.as_str(),
            priority: t.priority.as_str(),
            assigned_to_user_id: t.assigned_to_user_id,
            created_at: ts(t.created_at),
            updated_at: ts(t.updated_at),
            closed_at: t.closed_at.map(ts),
            client: ClientRef {
                id: t.client_id,
                email: t.client_email,
                name: t.client_name,
            },
            project: ProjectRef {
                id: t.project_id,
                name: t.project_name,
            },
            assigned_to,
        }
    }
}

pub fn stats_json(s: TicketStats) -> Value {
    json!({
        "pending": s.pending, "in_progress": s.in_progress, "on_hold": s.on_hold,
        "resolved": s.resolved, "closed": s.closed, "all": s.all,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageDto {
    id: Id,
    ticket_id: Id,
    sender_type: &'static str,
    sender_user_id: Option<Id>,
    sender_email: Option<String>,
    sender_name: Option<String>,
    body: String,
    html_body: Option<String>,
    message_type: &'static str,
    is_internal: bool,
    is_published: bool,
    attachment_ids: Vec<AttachmentRef>,
    created_at: String,
    updated_at: String,
    sender: Option<UserRef>,
}

#[derive(Serialize)]
struct AttachmentRef {
    id: Id,
    url: String,
}

impl MessageDto {
    pub fn new(m: Message, api_version: &str) -> Self {
        let sender = match (m.sender_user_id, m.agent_email, m.agent_name) {
            (Some(id), Some(email), Some(name)) => Some(UserRef { id, email, name }),
            _ => None,
        };
        Self {
            id: m.id,
            ticket_id: m.ticket_id,
            sender_type: m.sender_type.as_str(),
            sender_user_id: m.sender_user_id,
            sender_email: m.sender_email,
            sender_name: m.sender_name,
            body: m.body,
            html_body: m.html_body,
            message_type: m.message_type.as_str(),
            is_internal: m.is_internal,
            is_published: m.is_published,
            attachment_ids: m
                .attachment_ids
                .into_iter()
                .map(|id| AttachmentRef {
                    id,
                    url: format!("/api/{api_version}/attachments/file/{id}"),
                })
                .collect(),
            created_at: ts(m.created_at),
            updated_at: ts(m.updated_at),
            sender,
        }
    }
}
