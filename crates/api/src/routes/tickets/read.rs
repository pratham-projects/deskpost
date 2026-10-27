//! List, search, counts, get, update and delete of tickets.

use axum::extract::State;
use sd_core::status::{TicketStatus as Core, TransitionError};
use sd_db::tickets::{self, TicketFilter, TicketPatch};
use sd_db::{Id, Priority, TicketStatus, is_foreign_key_violation};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use super::dto::{TicketDto, stats_json};
use super::{Params, create::MAX_DESCRIPTION, create::valid_subject};
use crate::error::{ApiError, Envelope, Fields, Pagination};
use crate::extract::{ApiJson, ApiPath, AuthUser, project_admin};
use crate::state::AppState;

const MIN_SEARCH_CHARS: usize = 2;

pub async fn list(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    q: Params,
) -> Result<Envelope<Vec<TicketDto>>, ApiError> {
    let mut f = Fields::default();
    let filter = TicketFilter {
        project_id: q.uuid("projectId", &mut f),
        status: q.parse("status", &mut f),
        assigned_to: q.uuid("assignedToUserId", &mut f),
        priority: q.parse("priority", &mut f),
        unread_first: q.get("unreadFirst") == Some("true"),
    };
    let page = q.page(&mut f);
    f.finish()?;
    let (rows, total) = tickets::list(&s.pool, user.id, &filter, page).await?;
    // Stats ignore status/priority/assignee so the UI tabs keep their totals.
    let stats = tickets::stats(&s.pool, user.id, filter.project_id).await?;
    Ok(Envelope::ok(rows.into_iter().map(Into::into).collect())
        .paginated(Pagination::new(total, page.limit, page.offset))
        .stats(stats_json(stats)))
}

pub async fn search(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    q: Params,
) -> Result<Envelope<Vec<TicketDto>>, ApiError> {
    let mut f = Fields::default();
    let text = q.get("q").unwrap_or_default().trim();
    if text.chars().count() < MIN_SEARCH_CHARS {
        f.add("q", "Search needs at least 2 characters.");
    }
    let project_id = q.uuid("projectId", &mut f);
    let page = q.page(&mut f);
    f.finish()?;
    let (rows, total) = tickets::search(&s.pool, user.id, text, project_id, page).await?;
    Ok(
        Envelope::ok(rows.into_iter().map(Into::into).collect()).paginated(Pagination::new(
            total,
            page.limit,
            page.offset,
        )),
    )
}

pub async fn counts(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
) -> Result<Envelope<Value>, ApiError> {
    let (global, per_project) = tickets::counts(&s.pool, user.id).await?;
    let projects: serde_json::Map<String, Value> = per_project
        .into_iter()
        .map(|(id, stats)| (id.to_string(), stats_json(stats)))
        .collect();
    Ok(Envelope::ok(
        json!({ "global": stats_json(global), "projects": projects }),
    ))
}

pub async fn get(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(id): ApiPath<Id>,
) -> Result<Envelope<TicketDto>, ApiError> {
    let ticket = tickets::get(&s.pool, user.id, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Ticket"))?;
    Ok(Envelope::ok(ticket.into()))
}

/// `null` unassigns, absent leaves the assignee alone.
fn nullable<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<Id>>, D::Error> {
    Option::<Id>::deserialize(d).map(Some)
}

#[derive(Deserialize)]
pub struct PatchReq {
    subject: Option<String>,
    description: Option<String>,
    status: Option<String>,
    priority: Option<String>,
    #[serde(default, rename = "assignedToUserId", deserialize_with = "nullable")]
    assigned_to_user_id: Option<Option<Id>>,
}

fn core(s: TicketStatus) -> Core {
    match s {
        TicketStatus::Pending => Core::Pending,
        TicketStatus::InProgress => Core::InProgress,
        TicketStatus::OnHold => Core::OnHold,
        TicketStatus::Resolved => Core::Resolved,
        TicketStatus::Closed => Core::Closed,
    }
}

pub async fn update(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(id): ApiPath<Id>,
    ApiJson(req): ApiJson<PatchReq>,
) -> Result<Envelope<TicketDto>, ApiError> {
    let mut f = Fields::default();
    let mut patch = TicketPatch {
        assignee: req.assigned_to_user_id,
        ..TicketPatch::default()
    };
    if let Some(subject) = &req.subject {
        match valid_subject(subject) {
            Some(v) => patch.subject = Some(v),
            None => f.add("subject", "Subject must be 5 to 255 characters."),
        }
    }
    if let Some(d) = &req.description {
        let d = d.trim();
        if d.is_empty() || d.chars().count() > MAX_DESCRIPTION {
            f.add("description", "Description must be 1 to 20000 characters.");
        } else {
            patch.description = Some(d.to_string());
        }
    }
    if let Some(p) = &req.priority {
        match p.parse::<Priority>() {
            Ok(p) => patch.priority = Some(p),
            Err(_) => f.add("priority", "Priority must be low, normal, high or urgent."),
        }
    }
    let status = match &req.status {
        None => None,
        Some(v) => match v.parse::<TicketStatus>() {
            Ok(st) => Some(st),
            Err(_) => {
                f.add("status", "Unknown status.");
                None
            }
        },
    };
    if req.subject.is_none()
        && req.description.is_none()
        && req.priority.is_none()
        && req.status.is_none()
        && req.assigned_to_user_id.is_none()
    {
        f.add("body", "Nothing to update.");
    }
    f.finish()?;

    let current = tickets::get(&s.pool, user.id, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Ticket"))?;
    if let Some(to) = status {
        match core(current.status).transition(core(to)) {
            Ok(_) => patch.status = Some(to),
            // Re-sending the current status is a no-op, not an error (the UI sends whole forms).
            Err(TransitionError::Unchanged) => {}
            Err(TransitionError::ClosedIsFinal) => {
                return Err(ApiError::Conflict(
                    "A closed ticket can only be reopened to pending.".into(),
                ));
            }
        }
    }
    match tickets::update(&s.pool, user.id, id, &patch).await {
        Ok(Some(u)) => Ok(Envelope::ok(u.ticket.into())),
        Ok(None) => Err(ApiError::not_found("Ticket")),
        Err(e) if is_foreign_key_violation(&e) => {
            let mut f = Fields::default();
            f.add(
                "assignedToUserId",
                "Assignee must be a member of the ticket's project.",
            );
            Err(f.finish().unwrap_err())
        }
        Err(e) => Err(e.into()),
    }
}

pub async fn delete(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(id): ApiPath<Id>,
) -> Result<Envelope<Value>, ApiError> {
    let ticket = tickets::get(&s.pool, user.id, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Ticket"))?;
    project_admin(&s, user.id, ticket.project_id).await?;
    if !tickets::delete(&s.pool, user.id, id).await? {
        return Err(ApiError::not_found("Ticket"));
    }
    Ok(Envelope::ok(json!({ "success": true, "id": id })))
}
