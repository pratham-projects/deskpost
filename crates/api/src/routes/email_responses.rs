//! `/email-responses` (`docs/API.md` section 8): auto-replies per project and ticket event.

use axum::Router;
use axum::extract::State;
use axum::routing::{get, patch};
use sd_db::email_responses::{self, EmailResponse};
use sd_db::{EmailResponseStatus, Id, is_unique_violation};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::qparams::Params;
use crate::dto::ts;
use crate::error::{ApiError, Envelope, Fields};
use crate::extract::{ApiJson, ApiPath, AuthUser, project_admin, project_role};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/email-responses", get(list).post(create))
        .route("/email-responses/", get(list).post(create))
        .route("/email-responses/{id}", patch(update).delete(remove))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EmailResponseDto {
    id: Id,
    project_id: Id,
    status: &'static str,
    subject: String,
    body: String,
    is_enabled: bool,
    created_at: String,
    updated_at: String,
}

impl From<EmailResponse> for EmailResponseDto {
    fn from(r: EmailResponse) -> Self {
        Self {
            id: r.id,
            project_id: r.project_id,
            status: r.status.as_str(),
            subject: r.subject,
            body: r.body,
            is_enabled: r.is_enabled,
            created_at: ts(r.created_at),
            updated_at: ts(r.updated_at),
        }
    }
}

fn status_of(f: &mut Fields, s: &str) -> EmailResponseStatus {
    s.parse().unwrap_or_else(|_| {
        f.add("status", "Must be created or resolved.");
        EmailResponseStatus::Created
    })
}

fn subject(f: &mut Fields, s: &str) {
    if s.chars().count() > 255 || s.chars().any(|c| c == '\r' || c == '\n') {
        f.add(
            "subject",
            "Subject must be a single line of at most 255 characters.",
        );
    }
}

fn body(f: &mut Fields, b: &str) {
    if b.trim().is_empty() || b.chars().count() > 20_000 {
        f.add("body", "Body must be 1 to 20000 characters.");
    }
}

async fn list(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    q: Params,
) -> Result<Envelope<Vec<EmailResponseDto>>, ApiError> {
    let mut f = Fields::default();
    let project = q.uuid("projectId", &mut f);
    let status = q.get("status").map(|v| status_of(&mut f, v));
    f.finish()?;
    let project = project.unwrap_or_default();
    project_role(&s, user.id, project).await?;
    let rows = email_responses::list(&s.pool, user.id, project, status).await?;
    Ok(Envelope::ok(rows.into_iter().map(Into::into).collect()))
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct CreateReq {
    project_id: Option<Id>,
    status: String,
    subject: String,
    body: String,
    is_enabled: Option<bool>,
}

async fn create(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiJson(req): ApiJson<CreateReq>,
) -> Result<Envelope<EmailResponseDto>, ApiError> {
    let mut f = Fields::default();
    if req.project_id.is_none() {
        f.add("projectId", "A valid id is required.");
    }
    let status = status_of(&mut f, &req.status);
    subject(&mut f, &req.subject);
    body(&mut f, &req.body);
    f.finish()?;
    let project = req.project_id.unwrap_or_default();
    project_admin(&s, user.id, project).await?;
    let r = email_responses::create(
        &s.pool,
        user.id,
        project,
        status,
        req.subject.trim(),
        &req.body,
        req.is_enabled.unwrap_or(true),
    )
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            ApiError::Conflict("This event already has an email response.".into())
        } else {
            e.into()
        }
    })?
    .ok_or_else(|| ApiError::not_found("Project"))?;
    Ok(Envelope::created(r.into()))
}

fn nullable<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

/// `subject: null` clears the custom subject, like `""`.
#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct PatchReq {
    #[serde(deserialize_with = "nullable")]
    subject: Option<Option<String>>,
    body: Option<String>,
    is_enabled: Option<bool>,
}

/// Admin of the response's project; a member who is not admin gets 403, anyone else 404.
async fn admin_of(s: &AppState, user: Id, id: Id) -> Result<(), ApiError> {
    let project = email_responses::project_of(&s.pool, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Email response"))?;
    project_admin(s, user, project).await.map_err(|e| match e {
        ApiError::NotFound(_) => ApiError::not_found("Email response"),
        e => e,
    })
}

async fn update(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(id): ApiPath<Id>,
    ApiJson(req): ApiJson<PatchReq>,
) -> Result<Envelope<EmailResponseDto>, ApiError> {
    admin_of(&s, user.id, id).await?;
    let mut f = Fields::default();
    let new_subject = req.subject.map(Option::unwrap_or_default);
    if let Some(v) = &new_subject {
        subject(&mut f, v);
    }
    if let Some(b) = &req.body {
        body(&mut f, b);
    }
    f.finish()?;
    let r = email_responses::update(
        &s.pool,
        user.id,
        id,
        new_subject.as_deref().map(str::trim),
        req.body.as_deref(),
        req.is_enabled,
    )
    .await?
    .ok_or_else(|| ApiError::not_found("Email response"))?;
    Ok(Envelope::ok(r.into()))
}

async fn remove(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(id): ApiPath<Id>,
) -> Result<Envelope<Value>, ApiError> {
    admin_of(&s, user.id, id).await?;
    if !email_responses::delete(&s.pool, user.id, id).await? {
        return Err(ApiError::not_found("Email response"));
    }
    Ok(Envelope::ok(json!({ "success": true })))
}
