//! `/projects` (`docs/API.md` section 6): CRUD, archive, API key rotation, mail credentials view.

use axum::Router;
use axum::extract::State;
use axum::routing::{get, post};
use sd_core::api_key::generate_api_key;
use sd_db::projects::{self, MailSettings, NewProject, Project, ProjectPatch};
use sd_db::{Id, Status, constraint, is_unique_violation};
use sd_storage::{FsStorage, Storage};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::mail_guard;
use super::project_mail::MailFields;
use super::qparams::Params;
use crate::dto::ts;
use crate::error::{ApiError, Envelope, Fields, Pagination};
use crate::extract::{ApiJson, ApiPath, AuthUser, project_admin, project_role};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{id}", get(get_one).patch(update).delete(remove))
        .route("/{id}/regenerate-api-key", post(regenerate))
        .route("/{id}/credentials", get(credentials))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDto {
    id: Id,
    name: String,
    slug: String,
    domain: Option<String>,
    description: Option<String>,
    status: &'static str,
    is_archived: bool,
    api_key_prefix: String,
    owner_user_id: Id,
    reply_to_email: Option<String>,
    smtp_email: Option<String>,
    smtp_username: Option<String>,
    smtp_host: String,
    smtp_port: i32,
    has_smtp_password: bool,
    imap_host: Option<String>,
    imap_port: Option<i32>,
    imap_username: Option<String>,
    imap_folder: String,
    has_imap_password: bool,
    created_at: String,
    updated_at: String,
}

impl From<Project> for ProjectDto {
    fn from(p: Project) -> Self {
        Self {
            id: p.id,
            name: p.name,
            slug: p.slug,
            domain: p.domain,
            description: p.description,
            status: p.status.as_str(),
            is_archived: p.is_archived,
            api_key_prefix: p.api_key_prefix,
            owner_user_id: p.owner_user_id,
            reply_to_email: p.reply_to_email,
            smtp_email: p.smtp_email,
            smtp_username: p.smtp_username,
            smtp_host: p.smtp_host.unwrap_or_default(),
            smtp_port: p.smtp_port,
            has_smtp_password: p.has_smtp_password,
            imap_host: p.imap_host,
            imap_port: p.imap_port,
            imap_username: p.imap_username,
            imap_folder: p.imap_folder,
            has_imap_password: p.has_imap_password,
            created_at: ts(p.created_at),
            updated_at: ts(p.updated_at),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WithKey {
    #[serde(flatten)]
    project: ProjectDto,
    api_key: String,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct CreateReq {
    name: String,
    slug: String,
    domain: Option<String>,
    description: Option<String>,
    #[serde(flatten)]
    mail: MailFields,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct PatchReq {
    name: Option<String>,
    slug: Option<String>,
    domain: Option<String>,
    description: Option<String>,
    status: Option<String>,
    is_archived: Option<bool>,
    #[serde(flatten)]
    mail: MailFields,
}

fn check_name(f: &mut Fields, name: &str) -> String {
    let name = name.trim().to_string();
    if name.is_empty() || name.chars().count() > 255 || name.chars().any(char::is_control) {
        f.add("name", "Name must be 1 to 255 characters.");
    }
    name
}

fn check_slug(f: &mut Fields, slug: &str) -> String {
    let slug = slug.trim().to_string();
    let ok = (3..=255).contains(&slug.len())
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !ok {
        f.add(
            "slug",
            "Slug must be 3 to 255 characters of a-z, 0-9 and -.",
        );
    }
    slug
}

/// `""` clears. Anything else must look like `example.com`.
fn check_domain(f: &mut Fields, domain: Option<String>) -> Option<String> {
    domain.map(|d| {
        let d = d.trim().to_ascii_lowercase();
        if !d.is_empty() && (!d.contains('.') || mail_guard::check_host(&d, true).is_err()) {
            f.add("domain", "Enter a valid domain such as example.com.");
        }
        d
    })
}

fn check_description(f: &mut Fields, d: &Option<String>) {
    if d.as_deref().is_some_and(|d| d.chars().count() > 5000) {
        f.add("description", "Must be at most 5000 characters.");
    }
}

fn conflict(e: sd_db::Error) -> ApiError {
    if !is_unique_violation(&e) {
        return e.into();
    }
    ApiError::Conflict(
        match constraint(&e) {
            Some("projects_slug_key") => "This slug is already taken.",
            Some("projects_domain_key") => "This domain is already used by another project.",
            Some("projects_smtp_email_key") => "This mailbox address belongs to another project.",
            _ => "A project with these details already exists.",
        }
        .into(),
    )
}

async fn list(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    q: Params,
) -> Result<Envelope<Vec<ProjectDto>>, ApiError> {
    let mut f = Fields::default();
    let page = q.page(&mut f);
    let status = q.get("status").map(str::parse::<Status>).transpose();
    if status.is_err() {
        f.add("status", "Must be active, inactive or suspended.");
    }
    f.finish()?;
    let (rows, total) = projects::list(&s.pool, user.id, status.unwrap_or(None), page).await?;
    Ok(
        Envelope::ok(rows.into_iter().map(Into::into).collect()).paginated(Pagination::new(
            total,
            page.limit,
            page.offset,
        )),
    )
}

async fn create(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiJson(req): ApiJson<CreateReq>,
) -> Result<Envelope<WithKey>, ApiError> {
    let mut f = Fields::default();
    let name = check_name(&mut f, &req.name);
    let slug = check_slug(&mut f, &req.slug);
    let domain = check_domain(&mut f, req.domain);
    check_description(&mut f, &req.description);
    let mail = req.mail.validate(&mut f, s.config.allow_private_mail_hosts);
    f.finish()?;

    let (key, hash) = generate_api_key();
    let project = projects::create(
        &s.pool,
        user.id,
        &NewProject {
            name,
            slug,
            domain: domain.filter(|d| !d.is_empty()),
            description: req.description,
            api_key_hash: hash,
            api_key_prefix: key.chars().take(12).collect(),
        },
    )
    .await
    .map_err(conflict)?;
    let project = if mail.is_empty() {
        project
    } else {
        // The password ciphertext is bound to the project id, so this needs the row first. If the
        // mail half fails (mailbox address taken) the new project is removed again.
        let update = mail.into_update(&s.secrets, project.id);
        match projects::set_mail_settings(&s.pool, user.id, project.id, &update).await {
            Ok(Some(p)) => p,
            other => {
                projects::delete(&s.pool, user.id, project.id).await?;
                return Err(match other {
                    Err(e) => conflict(e),
                    _ => ApiError::Internal,
                });
            }
        }
    };
    Ok(Envelope::created(WithKey {
        project: project.into(),
        api_key: key,
    }))
}

async fn get_one(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(id): ApiPath<Id>,
) -> Result<Envelope<ProjectDto>, ApiError> {
    let p = projects::get(&s.pool, user.id, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Project"))?;
    Ok(Envelope::ok(p.into()))
}

async fn update(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(id): ApiPath<Id>,
    ApiJson(req): ApiJson<PatchReq>,
) -> Result<Envelope<ProjectDto>, ApiError> {
    project_admin(&s, user.id, id).await?;
    let mut f = Fields::default();
    let name = req.name.map(|n| check_name(&mut f, &n));
    let slug = req.slug.map(|v| check_slug(&mut f, &v));
    let domain = check_domain(&mut f, req.domain);
    check_description(&mut f, &req.description);
    let status = req.status.map(|v| {
        v.parse::<Status>().unwrap_or_else(|_| {
            f.add("status", "Must be active, inactive or suspended.");
            Status::Active
        })
    });
    let mail = req.mail.validate(&mut f, s.config.allow_private_mail_hosts);
    let current = projects::get_mail_settings(&s.pool, user.id, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Project"))?;
    mail.require_reentry(&current, &mut f);
    f.finish()?;

    let patch = ProjectPatch {
        name,
        slug,
        domain,
        description: req.description,
        status,
        is_archived: req.is_archived,
        auto_close_days: None,
    };
    let update = mail.into_update(&s.secrets, id);
    let p = projects::update_with_mail(&s.pool, user.id, id, &patch, &update)
        .await
        .map_err(conflict)?
        .ok_or_else(|| ApiError::not_found("Project"))?;
    Ok(Envelope::ok(p.into()))
}

async fn remove(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(id): ApiPath<Id>,
) -> Result<Envelope<Value>, ApiError> {
    project_role(&s, user.id, id).await?;
    let project = projects::get(&s.pool, user.id, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Project"))?;
    if project.owner_user_id != user.id {
        return Err(ApiError::Forbidden(
            "Only the project owner can delete it.".into(),
        ));
    }
    let keys = sd_db::attachments::storage_keys_for_project(&s.pool, user.id, id).await?;
    if !projects::delete(&s.pool, user.id, id).await? {
        return Err(ApiError::not_found("Project"));
    }
    // The rows are gone; a file that cannot be removed is only logged (nothing references it).
    match FsStorage::new(&s.config.storage_dir, s.config.upload_limit as u64).await {
        Ok(store) => {
            for key in keys {
                if let Err(e) = store.delete(&key).await {
                    tracing::warn!(error = %e, "attachment file not removed");
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "storage unavailable, attachment files kept"),
    }
    Ok(Envelope::ok(json!({ "success": true, "id": id })))
}

async fn regenerate(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(id): ApiPath<Id>,
) -> Result<Envelope<Value>, ApiError> {
    project_admin(&s, user.id, id).await?;
    let (key, hash) = generate_api_key();
    let prefix: String = key.chars().take(12).collect();
    let updated = projects::replace_api_key(&s.pool, user.id, id, &hash, &prefix)
        .await?
        .ok_or_else(|| ApiError::not_found("Project"))?;
    Ok(Envelope::ok(
        json!({ "id": id, "apiKey": key, "updatedAt": ts(updated) }),
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Credentials {
    email: Option<String>,
    username: Option<String>,
    smtp_host: Option<String>,
    smtp_port: i32,
    has_smtp_password: bool,
    imap_host: Option<String>,
    imap_port: Option<i32>,
    imap_username: Option<String>,
    imap_folder: String,
    has_imap_password: bool,
}

impl From<MailSettings> for Credentials {
    fn from(m: MailSettings) -> Self {
        Self {
            email: m.smtp_email,
            username: m.smtp_username,
            smtp_host: m.smtp_host,
            smtp_port: m.smtp_port,
            has_smtp_password: m.smtp_password_encrypted.is_some(),
            imap_host: m.imap_host,
            imap_port: m.imap_port,
            imap_username: m.imap_username,
            imap_folder: m.imap_folder,
            has_imap_password: m.imap_password_encrypted.is_some(),
        }
    }
}

async fn credentials(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(id): ApiPath<Id>,
) -> Result<Envelope<Credentials>, ApiError> {
    project_admin(&s, user.id, id).await?;
    let m = projects::get_mail_settings(&s.pool, user.id, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Project"))?;
    Ok(Envelope::ok(m.into()))
}
