//! `/projects/{id}/members` (`docs/API.md` section 6). The owner is a permanent admin member:
//! it cannot be removed or demoted (409).

use axum::Router;
use axum::extract::State;
use axum::routing::get;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use sd_db::members::{self, Member, MemberPatch};
use sd_db::{Id, Role, is_unique_violation, projects, users};

use super::qparams::Params;
use crate::dto::ts;
use crate::error::{ApiError, Envelope, Fields};
use crate::extract::{ApiJson, ApiPath, AuthUser, project_admin, project_role};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{id}/members", get(list).post(add))
        .route("/{id}/members/search", get(search))
        .route(
            "/{id}/members/{user_id}",
            axum::routing::patch(update).delete(remove),
        )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MemberDto {
    id: Id,
    project_id: Id,
    user_id: Id,
    role: &'static str,
    should_notify: bool,
    can_view_all_tickets: bool,
    can_manage_templates: bool,
    created_at: String,
    updated_at: String,
    user: Value,
}

impl From<Member> for MemberDto {
    fn from(m: Member) -> Self {
        Self {
            id: m.id,
            project_id: m.project_id,
            user_id: m.user_id,
            role: m.role.as_str(),
            should_notify: m.should_notify,
            can_view_all_tickets: m.can_view_all_tickets,
            can_manage_templates: m.can_manage_templates,
            created_at: ts(m.created_at),
            updated_at: ts(m.updated_at),
            user: json!({ "id": m.user_id, "email": m.user_email, "name": m.user_name }),
        }
    }
}

fn parse_role(f: &mut Fields, role: &str) -> Role {
    role.parse().unwrap_or_else(|_| {
        f.add("role", "Must be admin or agent.");
        Role::Agent
    })
}

async fn list(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(id): ApiPath<Id>,
) -> Result<Envelope<Vec<MemberDto>>, ApiError> {
    project_role(&s, user.id, id).await?;
    let rows = members::list(&s.pool, user.id, id).await?;
    Ok(Envelope::ok(rows.into_iter().map(Into::into).collect()))
}

async fn search(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(id): ApiPath<Id>,
    q: Params,
) -> Result<Envelope<Vec<MemberDto>>, ApiError> {
    project_role(&s, user.id, id).await?;
    let mut f = Fields::default();
    let page = q.page(&mut f);
    f.finish()?;
    let rows = members::search(&s.pool, user.id, id, q.get("q").unwrap_or(""), page).await?;
    Ok(Envelope::ok(rows.into_iter().map(Into::into).collect()))
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct AddReq {
    user_id: Option<Id>,
    role: Option<String>,
    should_notify: Option<bool>,
}

async fn add(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(id): ApiPath<Id>,
    ApiJson(req): ApiJson<AddReq>,
) -> Result<Envelope<MemberDto>, ApiError> {
    project_admin(&s, user.id, id).await?;
    let mut f = Fields::default();
    if req.user_id.is_none() {
        f.add("userId", "A user id is required.");
    }
    let role = req.role.map_or(Role::Agent, |r| parse_role(&mut f, &r));
    f.finish()?;
    let target = req.user_id.unwrap_or_default();
    // Only users the actor can already see, and only active ones: no UUID probing, no suspended members.
    match users::get_visible(&s.pool, user.id, target).await? {
        Some(u) if u.status == sd_db::Status::Active => {}
        _ => return Err(ApiError::not_found("User")),
    }
    let m = members::add(
        &s.pool,
        user.id,
        id,
        target,
        role,
        req.should_notify.unwrap_or(false),
    )
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            ApiError::Conflict("This user is already a member.".into())
        } else {
            e.into()
        }
    })?
    .ok_or_else(|| ApiError::not_found("Project"))?;
    Ok(Envelope::created(m.into()))
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct PatchReq {
    role: Option<String>,
    should_notify: Option<bool>,
    can_view_all_tickets: Option<bool>,
    can_manage_templates: Option<bool>,
}

async fn is_owner(s: &AppState, actor: Id, project: Id, target: Id) -> Result<bool, ApiError> {
    let p = projects::get(&s.pool, actor, project)
        .await?
        .ok_or_else(|| ApiError::not_found("Project"))?;
    Ok(p.owner_user_id == target)
}

async fn update(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath((id, target)): ApiPath<(Id, Id)>,
    ApiJson(req): ApiJson<PatchReq>,
) -> Result<Envelope<MemberDto>, ApiError> {
    project_admin(&s, user.id, id).await?;
    let mut f = Fields::default();
    let role = req.role.map(|r| parse_role(&mut f, &r));
    f.finish()?;
    if role.is_some_and(|r| r != Role::Admin) && is_owner(&s, user.id, id, target).await? {
        return Err(ApiError::Conflict(
            "The project owner must stay an admin.".into(),
        ));
    }
    let patch = MemberPatch {
        role,
        should_notify: req.should_notify,
        can_view_all_tickets: req.can_view_all_tickets,
        can_manage_templates: req.can_manage_templates,
    };
    let m = members::update(&s.pool, user.id, id, target, &patch)
        .await?
        .ok_or_else(|| ApiError::not_found("Member"))?;
    Ok(Envelope::ok(m.into()))
}

async fn remove(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath((id, target)): ApiPath<(Id, Id)>,
) -> Result<Envelope<Value>, ApiError> {
    let role = project_role(&s, user.id, id).await?;
    if role != Role::Admin && user.id != target {
        return Err(ApiError::Forbidden("Project admin access required.".into()));
    }
    if is_owner(&s, user.id, id, target).await? {
        return Err(ApiError::Conflict(
            "The project owner cannot be removed.".into(),
        ));
    }
    if !members::remove(&s.pool, user.id, id, target).await? {
        return Err(ApiError::not_found("Member"));
    }
    Ok(Envelope::ok(json!({ "success": true })))
}
