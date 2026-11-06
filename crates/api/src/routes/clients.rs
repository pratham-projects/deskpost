//! `/clients` (read only). The TS CRUD is dropped (`docs/API.md` deviation 9): clients are created
//! by tickets and inbound mail, one per (project, email). Agents can look them up.

use axum::Router;
use axum::extract::State;
use axum::routing::get;
use sd_db::Id;
use sd_db::clients::{self, Client};
use serde::Serialize;

use super::qparams::Params;
use crate::dto::ts;
use crate::error::{ApiError, Envelope, Fields};
use crate::extract::{ApiPath, AuthUser, project_role};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/clients", get(list))
        .route("/clients/", get(list))
        .route("/clients/{id}", get(get_one))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientDto {
    id: Id,
    project_id: Id,
    email: String,
    name: Option<String>,
    phone: Option<String>,
    created_at: String,
    updated_at: String,
}

impl From<Client> for ClientDto {
    fn from(c: Client) -> Self {
        Self {
            id: c.id,
            project_id: c.project_id,
            email: c.email,
            name: c.name,
            phone: c.phone,
            created_at: ts(c.created_at),
            updated_at: ts(c.updated_at),
        }
    }
}

async fn list(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    q: Params,
) -> Result<Envelope<Vec<ClientDto>>, ApiError> {
    let mut f = Fields::default();
    let project = q.uuid("projectId", &mut f);
    let page = q.page(&mut f);
    f.finish()?;
    let project = project.unwrap_or_default();
    project_role(&s, user.id, project).await?;
    let needle = q.get("q").filter(|q| !q.is_empty());
    let rows = clients::list(&s.pool, user.id, project, needle, page).await?;
    Ok(Envelope::ok(rows.into_iter().map(Into::into).collect()))
}

async fn get_one(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(id): ApiPath<Id>,
) -> Result<Envelope<ClientDto>, ApiError> {
    let c = clients::get(&s.pool, user.id, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Client"))?;
    Ok(Envelope::ok(c.into()))
}
