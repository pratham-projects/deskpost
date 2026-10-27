//! `/template-responses` (`docs/API.md` section 8): canned replies, per project.

use axum::Router;
use axum::extract::State;
use axum::routing::{get, patch};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use sd_db::Id;
use sd_db::templates::{self, Template};

use super::qparams::Params;
use crate::dto::ts;
use crate::error::{ApiError, Envelope, Fields, Pagination};
use crate::extract::{ApiJson, ApiPath, AuthUser, project_role};
use crate::state::AppState;

const MAX_BODY_CHARS: usize = 20_000;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/template-responses", get(list).post(create))
        .route("/template-responses/", get(list).post(create))
        .route("/template-responses/search", get(search))
        .route("/template-responses/{id}", patch(update).delete(remove))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TemplateDto {
    id: Id,
    project_id: Id,
    title: String,
    body: String,
    created_at: String,
    updated_at: String,
}

impl From<Template> for TemplateDto {
    fn from(t: Template) -> Self {
        Self {
            id: t.id,
            project_id: t.project_id,
            title: t.title,
            body: t.body,
            created_at: ts(t.created_at),
            updated_at: ts(t.updated_at),
        }
    }
}

fn title(f: &mut Fields, t: &str) -> String {
    let t = t.trim().to_string();
    if t.is_empty() || t.chars().count() > 255 {
        f.add("title", "Title must be 1 to 255 characters.");
    }
    t
}

fn body(f: &mut Fields, b: &str) {
    if b.trim().is_empty() || b.chars().count() > MAX_BODY_CHARS {
        f.add("body", "Body must be 1 to 20000 characters.");
    }
}

async fn page_of(
    s: &AppState,
    user: Id,
    q: &Params,
    search: bool,
) -> Result<Envelope<Vec<TemplateDto>>, ApiError> {
    let mut f = Fields::default();
    let project = q.uuid("projectId", &mut f);
    let page = q.page(&mut f);
    f.finish()?;
    let project = project.unwrap_or_default();
    project_role(s, user, project).await?;
    let needle = if search { q.get("q") } else { None };
    let (rows, total) = templates::list(&s.pool, user, project, needle, page).await?;
    Ok(
        Envelope::ok(rows.into_iter().map(Into::into).collect()).paginated(Pagination::new(
            total,
            page.limit,
            page.offset,
        )),
    )
}

async fn list(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    q: Params,
) -> Result<Envelope<Vec<TemplateDto>>, ApiError> {
    page_of(&s, user.id, &q, false).await
}

async fn search(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    q: Params,
) -> Result<Envelope<Vec<TemplateDto>>, ApiError> {
    page_of(&s, user.id, &q, true).await
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct CreateReq {
    project_id: Option<Id>,
    title: String,
    body: String,
}

async fn create(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiJson(req): ApiJson<CreateReq>,
) -> Result<Envelope<TemplateDto>, ApiError> {
    let mut f = Fields::default();
    if req.project_id.is_none() {
        f.add("projectId", "A valid id is required.");
    }
    let t = title(&mut f, &req.title);
    body(&mut f, &req.body);
    f.finish()?;
    let project = req.project_id.unwrap_or_default();
    project_role(&s, user.id, project).await?;
    let created = templates::create(&s.pool, user.id, project, &t, &req.body)
        .await?
        .ok_or_else(|| {
            ApiError::Forbidden("You cannot manage templates in this project.".into())
        })?;
    Ok(Envelope::created(created.into()))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PatchReq {
    title: Option<String>,
    body: Option<String>,
}

/// Member of the template's project, else 404 (an unknown id looks the same).
async fn visible(s: &AppState, user: Id, id: Id) -> Result<Template, ApiError> {
    templates::get(&s.pool, user, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Template"))
}

fn can_manage_msg() -> ApiError {
    ApiError::Forbidden("You cannot manage templates in this project.".into())
}

async fn update(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(id): ApiPath<Id>,
    ApiJson(req): ApiJson<PatchReq>,
) -> Result<Envelope<TemplateDto>, ApiError> {
    visible(&s, user.id, id).await?;
    let mut f = Fields::default();
    let t = req.title.as_deref().map(|t| title(&mut f, t));
    if let Some(b) = &req.body {
        body(&mut f, b);
    }
    f.finish()?;
    let updated = templates::update(&s.pool, user.id, id, t.as_deref(), req.body.as_deref())
        .await?
        .ok_or_else(can_manage_msg)?;
    Ok(Envelope::ok(updated.into()))
}

async fn remove(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(id): ApiPath<Id>,
) -> Result<Envelope<Value>, ApiError> {
    visible(&s, user.id, id).await?;
    if !templates::delete(&s.pool, user.id, id).await? {
        return Err(can_manage_msg());
    }
    Ok(Envelope::ok(json!({ "success": true })))
}
