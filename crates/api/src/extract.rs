//! Extractors. Every rejection is an [`ApiError`], so no client mistake ever answers with axum's
//! plain-text defaults.

use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{FromRequest, FromRequestParts, Path, Request};
use axum::http::StatusCode;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use dp_db::users::User;
use dp_db::{Id, Role, Status, members};
use serde::de::DeserializeOwned;

use crate::error::ApiError;
use crate::jwt::JwtError;
use crate::state::AppState;

/// `Json<T>` whose failures use the envelope: 400 malformed, 413 too large, 415 wrong content
/// type, 422 wrong shape.
pub struct ApiJson<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for ApiJson<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, ApiError> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(v)) => Ok(Self(v)),
            Err(rej) => Err(json_rejection(&rej)),
        }
    }
}

fn json_rejection(rej: &JsonRejection) -> ApiError {
    match rej.status() {
        StatusCode::PAYLOAD_TOO_LARGE => ApiError::PayloadTooLarge,
        StatusCode::UNSUPPORTED_MEDIA_TYPE => ApiError::UnsupportedMediaType,
        StatusCode::UNPROCESSABLE_ENTITY => ApiError::Validation(
            [(
                "body".to_string(),
                "Missing required fields or wrong types.".to_string(),
            )]
            .into(),
        ),
        _ => ApiError::BadRequest("Malformed request body.".into()),
    }
}

/// `Path<T>` whose failures (bad UUID, ...) are `400 BAD_REQUEST` envelopes.
pub struct ApiPath<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned + Send> FromRequestParts<S> for ApiPath<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        match Path::<T>::from_request_parts(parts, state).await {
            Ok(Path(v)) => Ok(Self(v)),
            Err(PathRejection::MissingPathParams(e)) => Err(ApiError::internal(e)),
            Err(_) => Err(ApiError::BadRequest("Invalid path parameter.".into())),
        }
    }
}

/// A signed-in, active user. Reads the Bearer token and loads the user, so a deactivated account
/// loses access immediately instead of when its token expires.
pub struct AuthUser(pub User);

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let header = parts
            .headers
            .get(AUTHORIZATION)
            .ok_or(ApiError::TokenMissing)?
            .to_str()
            .map_err(|_| ApiError::TokenInvalid)?;
        let token = header
            .split_once(' ')
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
            .map(|(_, t)| t)
            .ok_or(ApiError::TokenInvalid)?;
        let now = chrono::Utc::now().timestamp();
        let claims = state.jwt.verify(token, now).map_err(|e| match e {
            JwtError::Expired => ApiError::TokenExpired,
            JwtError::Invalid => ApiError::TokenInvalid,
        })?;
        let user = dp_db::users::get(&state.pool, claims.sub)
            .await?
            .filter(|u| u.status == Status::Active)
            .ok_or(ApiError::TokenInvalid)?;
        Ok(Self(user))
    }
}

/// A signed-in user with the global `admin` role (user management only; project data is decided
/// by membership, see [`project_role`]).
pub struct GlobalAdmin(pub User);

impl FromRequestParts<AppState> for GlobalAdmin {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let AuthUser(user) = AuthUser::from_request_parts(parts, state).await?;
        if user.role != Role::Admin {
            return Err(ApiError::Forbidden("Administrator access required.".into()));
        }
        Ok(Self(user))
    }
}

/// The caller's role in a project; 404 when they are not a member (never leak existence).
pub async fn project_role(state: &AppState, user: Id, project: Id) -> Result<Role, ApiError> {
    members::role_of(&state.pool, user, project)
        .await?
        .ok_or_else(|| ApiError::not_found("Project"))
}

/// Member with role `admin` (404 non-member, 403 plain agent).
pub async fn project_admin(state: &AppState, user: Id, project: Id) -> Result<(), ApiError> {
    match project_role(state, user, project).await? {
        Role::Admin => Ok(()),
        Role::Agent => Err(ApiError::Forbidden("Project admin access required.".into())),
    }
}
