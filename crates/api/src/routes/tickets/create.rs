//! `POST /tickets/admin` (agent) and `POST /tickets` (public widget). Both take multipart text
//! fields; file parts are the attachments task (R23) and are skipped here.

use std::collections::HashMap;

use axum::extract::State;
use axum::extract::multipart::{Multipart, MultipartError, MultipartRejection};
use axum::http::{HeaderMap, StatusCode};
use sd_core::api_key::hash_api_key;
use sd_core::email_addr::is_valid_email;
use sd_db::tickets::{self, Access, NewTicket};
use sd_db::{Id, Priority, TicketSource};

use super::dto::TicketDto;
use crate::client_ip::ClientIp;
use crate::error::{ApiError, Envelope, Fields};
use crate::extract::{AuthUser, project_role};
use crate::ratelimit::Rule;
use crate::state::AppState;

pub const MAX_DESCRIPTION: usize = 20_000;
const MAX_PARTS: usize = 40;
const TEXT_FIELDS: [&str; 6] = [
    "projectId",
    "clientEmail",
    "clientName",
    "subject",
    "description",
    "priority",
];

/// Trimmed subject of 5..=255 characters.
pub fn valid_subject(s: &str) -> Option<String> {
    let s = s.trim();
    (5..=255)
        .contains(&s.chars().count())
        .then(|| s.to_string())
}

fn multipart_error(e: &MultipartError) -> ApiError {
    match e.status() {
        StatusCode::PAYLOAD_TOO_LARGE => ApiError::PayloadTooLarge,
        _ => ApiError::BadRequest("Malformed multipart body.".into()),
    }
}

/// The known text fields of the form; the first occurrence of a field wins.
async fn read_fields(
    mp: Result<Multipart, MultipartRejection>,
) -> Result<HashMap<String, String>, ApiError> {
    // Anything but a well-formed multipart content type (wrong type, missing boundary) is a 415.
    let mut mp = mp.map_err(|_| ApiError::UnsupportedMediaType)?;
    let mut fields = HashMap::new();
    for _ in 0..MAX_PARTS {
        let Some(field) = mp.next_field().await.map_err(|e| multipart_error(&e))? else {
            return Ok(fields);
        };
        let Some(name) = field
            .name()
            .filter(|n| TEXT_FIELDS.contains(n))
            .map(str::to_owned)
        else {
            continue;
        };
        let text = field.text().await.map_err(|e| multipart_error(&e))?;
        fields.entry(name).or_insert(text);
    }
    Err(ApiError::BadRequest("Too many form fields.".into()))
}

/// Validated form: the ticket to create with `source` and `created_by` still unset.
fn validate(fields: &HashMap<String, String>) -> Result<NewTicket, ApiError> {
    let get = |k: &str| fields.get(k).map(|v| v.trim()).unwrap_or_default();
    let mut f = Fields::default();
    let project_id = get("projectId").parse::<Id>().unwrap_or_else(|_| {
        f.add("projectId", "projectId must be a UUID.");
        Id::nil()
    });
    let email = get("clientEmail").to_ascii_lowercase();
    if !is_valid_email(&email) {
        f.add("clientEmail", "Enter a valid email address.");
    }
    let name = get("clientName");
    if name.chars().count() > 255 {
        f.add("clientName", "Name must be at most 255 characters.");
    }
    let subject = valid_subject(get("subject")).unwrap_or_else(|| {
        f.add("subject", "Subject must be 5 to 255 characters.");
        String::new()
    });
    let description = get("description");
    if description.is_empty() || description.chars().count() > MAX_DESCRIPTION {
        f.add("description", "Description must be 1 to 20000 characters.");
    }
    let priority = match get("priority") {
        "" => Priority::Normal,
        p => p.parse().unwrap_or_else(|_| {
            f.add("priority", "Priority must be low, normal, high or urgent.");
            Priority::Normal
        }),
    };
    f.finish()?;
    let client_name = match name {
        "" => email.split('@').next().unwrap_or_default().to_string(),
        n => n.to_string(),
    };
    Ok(NewTicket {
        project_id,
        client_email: email,
        client_name: Some(client_name),
        subject,
        description: Some(description.to_string()),
        priority,
        source: TicketSource::Admin,
        created_by: None,
        first_message: description.to_string(),
    })
}

pub async fn admin(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    mp: Result<Multipart, MultipartRejection>,
) -> Result<Envelope<TicketDto>, ApiError> {
    let mut new = validate(&read_fields(mp).await?)?;
    project_role(&s, user.id, new.project_id).await?;
    new.created_by = Some(user.id);
    let ticket = tickets::create(&s.pool, &Access::Member(user.id), &new)
        .await?
        .ok_or_else(|| ApiError::not_found("Project"))?;
    Ok(Envelope::created(ticket.into()))
}

/// Public widget. No cookies are read or set; the project API key is the only credential, and a
/// wrong project id gets the same 401 as a wrong key.
pub async fn widget(
    State(s): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    mp: Result<Multipart, MultipartRejection>,
) -> Result<Envelope<TicketDto>, ApiError> {
    s.limiter.check(Rule::Public, ip)?;
    let key = headers
        .get("x-project-key")
        .and_then(|v| v.to_str().ok())
        .filter(|k| !k.is_empty())
        .ok_or(ApiError::TokenInvalid)?;
    let mut new = validate(&read_fields(mp).await?)?;
    new.source = TicketSource::Web;
    let access = Access::ApiKeyHash(hash_api_key(key));
    let ticket = tickets::create(&s.pool, &access, &new)
        .await?
        .ok_or(ApiError::TokenInvalid)?;
    Ok(Envelope::created(ticket.into()))
}
