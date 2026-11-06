//! `/attachments`: upload (raw and multipart), list, signed download links, download, delete.
//! Files live in `sd-storage`; the ticket's project decides who may touch them.

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::rejection::BytesRejection;
use axum::extract::{DefaultBodyLimit, FromRequestParts, Multipart, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::{get, post, put};
use sd_core::content_type::{self, normalize};
use sd_core::filename::sanitize_filename;
use sd_db::messages::{NewAgentMessage, Posted};
use sd_db::{Id, attachments, mailq, tickets};
use sd_storage::{Disposition, PutMeta, Storage};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

use crate::dto::ts;
use crate::error::{ApiError, Envelope, Fields};
use crate::extract::{ApiPath, AuthUser};
use crate::state::AppState;

const MAX_MESSAGE_CHARS: usize = 50_000;

/// `body_limit` is the config's upload cap for the whole request.
pub fn router(body_limit: usize) -> Router<AppState> {
    Router::new()
        .route("/{ticket_id}", put(upload_one))
        .route("/{ticket_id}/multiple", post(upload_many))
        .route("/ticket/{ticket_id}", get(list))
        .route("/file/{id}", get(download).delete(remove))
        .route("/file/{id}/url", get(signed_url))
        .layer(DefaultBodyLimit::max(body_limit))
}

/// Types a ticket upload may have: images (no SVG), PDF, text, office documents, archives.
pub fn upload_type_allowed(ct: &str) -> bool {
    const EXACT: [&str; 14] = [
        "image/png",
        "image/jpeg",
        "image/gif",
        "image/webp",
        "image/avif",
        "image/bmp",
        "application/pdf",
        "text/plain",
        "text/csv",
        "application/zip",
        "application/x-zip-compressed",
        "application/msword",
        "application/vnd.ms-excel",
        "application/vnd.ms-powerpoint",
    ];
    const PREFIX: [&str; 2] = [
        "application/vnd.openxmlformats-officedocument.",
        "application/vnd.oasis.opendocument.",
    ];
    EXACT.contains(&ct) || PREFIX.iter().any(|p| ct.starts_with(p))
}

struct Upload {
    filename: String,
    content_type: String,
    bytes: Vec<u8>,
}

fn dto(a: &attachments::Attachment) -> Value {
    json!({
        "id": a.id, "ticketId": a.ticket_id, "messageId": a.message_id, "filename": a.filename,
        "mimeType": a.mime_type, "sizeBytes": a.size_bytes, "createdAt": ts(a.created_at),
    })
}

fn too_large(msg: &str) -> ApiError {
    tracing::debug!(reason = msg, "upload refused");
    ApiError::PayloadTooLarge
}

/// Caps and type checks for a whole request, before anything is written.
fn validate(state: &AppState, files: &[Upload], message: Option<&str>) -> Result<(), ApiError> {
    let cfg = &state.config.mail;
    let mut f = Fields::default();
    if files.is_empty() {
        return Err(ApiError::BadRequest("No file was uploaded.".into()));
    }
    if files.len() > cfg.max_files {
        return Err(too_large("too many files"));
    }
    if files.iter().map(|u| u.bytes.len()).sum::<usize>() > cfg.max_request_bytes {
        return Err(too_large("request total"));
    }
    if files.iter().any(|u| u.bytes.len() > cfg.max_file_bytes) {
        return Err(too_large("file size"));
    }
    if files.iter().any(|u| u.bytes.is_empty()) {
        return Err(ApiError::BadRequest("Empty files are not accepted.".into()));
    }
    if files.iter().any(|u| !upload_type_allowed(&u.content_type)) {
        return Err(ApiError::UnsupportedMediaType);
    }
    if message.is_some_and(|m| m.chars().count() > MAX_MESSAGE_CHARS) {
        f.add("message", "At most 50000 characters.");
    }
    f.finish()
}

/// Store the files, send the mail if asked, then save the message. Any failure removes what was written.
async fn post_files(
    state: &AppState,
    user: Id,
    ticket_id: Id,
    files: Vec<Upload>,
    message: Option<String>,
    send_email: bool,
) -> Result<(Vec<Id>, Id), ApiError> {
    let ticket = tickets::get(&state.pool, user, ticket_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Ticket"))?;
    validate(state, &files, message.as_deref())?;
    let fs = state.store.fs().await?;

    let mut ids: Vec<Id> = Vec::new();
    let mut keys: Vec<String> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut failure = None;
    for u in &files {
        let filename = sanitize_filename(&u.filename);
        let ct = normalize(&u.content_type).unwrap_or_default();
        let meta = PutMeta {
            original_filename: Some(filename.clone()),
            content_type: Some(ct.clone()),
        };
        let obj = match fs.put(&u.bytes[..], meta).await {
            Ok(o) => o,
            Err(sd_storage::Error::TooLarge { .. }) => {
                failure = Some(too_large("storage cap"));
                break;
            }
            Err(e) => {
                failure = Some(ApiError::internal(e));
                break;
            }
        };
        keys.push(obj.key.clone());
        let new = attachments::NewAttachment {
            filename: filename.clone(),
            mime_type: ct,
            size_bytes: obj.size as i64,
            storage_key: obj.key,
            message_id: None,
        };
        match attachments::create(&state.pool, user, ticket_id, &new).await {
            Ok(attachments::Stored::Ok(a)) => {
                ids.push(a.id);
                names.push(filename);
            }
            Ok(attachments::Stored::QuotaExceeded) => {
                failure = Some(too_large("project storage quota"));
                break;
            }
            Ok(attachments::Stored::NoAccess) => {
                failure = Some(ApiError::not_found("Ticket"));
                break;
            }
            Err(e) => {
                failure = Some(e.into());
                break;
            }
        }
    }

    let result = match failure {
        Some(e) => Err(e),
        None => save_message(state, user, &ticket, &ids, &names, message, send_email).await,
    };
    match result {
        Ok(message_id) => Ok((ids, message_id)),
        Err(e) => {
            let _ = mailq::delete_attachments(&state.pool, ticket_id, &ids).await;
            for k in keys {
                let _ = fs.delete(&k).await;
            }
            Err(e)
        }
    }
}

async fn save_message(
    state: &AppState,
    user: Id,
    ticket: &tickets::Ticket,
    ids: &[Id],
    names: &[String],
    message: Option<String>,
    send_email: bool,
) -> Result<Id, ApiError> {
    let body = match message
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
    {
        Some(m) => m,
        None if names.len() == 1 => format!("Sent an attachment: {}", names[0]),
        None => format!("Sent {} attachments", names.len()),
    };
    let new = NewAgentMessage {
        body,
        attachment_ids: ids.to_vec(),
        ..Default::default()
    };
    let message = match sd_db::messages::post_agent(&state.pool, user, ticket.id, &new).await? {
        Posted::Ok(m) => m,
        Posted::NoAccess => return Err(ApiError::not_found("Ticket")),
        Posted::BadAttachments => {
            return Err(ApiError::internal("attachments could not be linked"));
        }
    };
    // Same rule as `/messages`: mail goes out after the commit and a failure never fails the upload.
    if send_email {
        crate::routes::tickets::deliver_reply(state, user, &message).await;
    }
    Ok(message.id)
}

fn header_str<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

fn flag(h: &HeaderMap, name: &str) -> bool {
    header_str(h, name).is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

async fn upload_one(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(ticket_id): ApiPath<Id>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Envelope<Value>, ApiError> {
    let body = body.map_err(|_| ApiError::PayloadTooLarge)?;
    let upload = Upload {
        filename: header_str(&headers, "x-filename")
            .unwrap_or("file")
            .to_string(),
        content_type: normalize(header_str(&headers, "content-type").unwrap_or_default())
            .unwrap_or_default(),
        bytes: body.to_vec(),
    };
    let message = header_str(&headers, "x-message").map(str::to_string);
    let (ids, message_id) = post_files(
        &s,
        user.id,
        ticket_id,
        vec![upload],
        message,
        flag(&headers, "x-send-email"),
    )
    .await?;
    Ok(Envelope::created(
        json!({ "attachmentId": ids[0], "messageId": message_id }),
    ))
}

async fn upload_many(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(ticket_id): ApiPath<Id>,
    mut form: Multipart,
) -> Result<Envelope<Value>, ApiError> {
    let bad = |e: axum::extract::multipart::MultipartError| {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::PayloadTooLarge
        } else {
            ApiError::BadRequest("Malformed multipart body.".into())
        }
    };
    let (mut files, mut message, mut send_email) = (Vec::new(), None, false);
    while let Some(field) = form.next_field().await.map_err(bad)? {
        match field.name().unwrap_or_default() {
            "files" => {
                if files.len() >= s.config.mail.max_files {
                    return Err(too_large("too many files"));
                }
                let filename = field.file_name().unwrap_or("file").to_string();
                let content_type =
                    normalize(field.content_type().unwrap_or_default()).unwrap_or_default();
                let bytes = field.bytes().await.map_err(bad)?.to_vec();
                files.push(Upload {
                    filename,
                    content_type,
                    bytes,
                });
            }
            "message" => message = Some(field.text().await.map_err(bad)?),
            "sendEmail" => {
                send_email = field
                    .text()
                    .await
                    .map_err(bad)?
                    .eq_ignore_ascii_case("true")
            }
            _ => {}
        }
    }
    let (ids, message_id) = post_files(&s, user.id, ticket_id, files, message, send_email).await?;
    Ok(Envelope::created(
        json!({ "attachmentIds": ids, "messageId": message_id }),
    ))
}

async fn list(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(ticket_id): ApiPath<Id>,
) -> Result<Envelope<Vec<Value>>, ApiError> {
    let rows = attachments::list_for_ticket(&s.pool, user.id, ticket_id)
        .await?
        .ok_or_else(|| ApiError::not_found("Ticket"))?;
    Ok(Envelope::ok(rows.iter().map(dto).collect()))
}

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

fn now_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

fn inline_ok(mime: &str) -> bool {
    content_type::classify(mime) == content_type::Disposition::Inline
}

/// `Content-Disposition` with an ASCII fallback and an RFC 5987 name; nothing from the stored
/// filename can break out of the quotes.
fn disposition_header(inline: bool, filename: &str) -> String {
    let clean = sanitize_filename(filename);
    let ascii: String = clean
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && !matches!(c, '"' | '\\' | '%' | ';') || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut pct = String::new();
    for b in clean.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            pct.push(b as char);
        } else {
            pct.push_str(&format!("%{b:02X}"));
        }
    }
    let kind = if inline { "inline" } else { "attachment" };
    format!("{kind}; filename=\"{ascii}\"; filename*=UTF-8''{pct}")
}

/// Bearer token, or a signed `?token=` minted by [`signed_url`] (for `<img src>` and downloads).
async fn download(
    State(s): State<AppState>,
    ApiPath(id): ApiPath<Id>,
    Query(q): Query<TokenQuery>,
    mut parts: Parts,
) -> Result<Response, ApiError> {
    let (att, token_inline) = match q.token {
        Some(token) => {
            let att = mailq::attachment_system(&s.pool, id)
                .await?
                .ok_or(ApiError::TokenInvalid)?;
            let d = s
                .store
                .signer
                .verify(&att.storage_key, &token, now_secs())
                .map_err(|_| ApiError::TokenInvalid)?;
            (att, d == Disposition::Inline)
        }
        None => {
            let AuthUser(user) = AuthUser::from_request_parts(&mut parts, &s).await?;
            let att = attachments::get(&s.pool, user.id, id)
                .await?
                .ok_or_else(|| ApiError::not_found("Attachment"))?;
            (att, true)
        }
    };
    let fs = s.store.fs().await?;
    let (reader, _) = fs.get(&att.storage_key).await.map_err(|e| match e {
        sd_storage::Error::NotFound => ApiError::not_found("Attachment"),
        e => ApiError::internal(e),
    })?;
    // Streamed, never held in memory; the length is the recorded size, so a file that changed on
    // disk cannot be served longer than what was uploaded.
    let size = att.size_bytes.max(0) as u64;
    let inline = token_inline && inline_ok(&att.mime_type);
    let mut res = Response::new(Body::from_stream(tokio_util::io::ReaderStream::new(
        reader.take(size),
    )));
    let h = res.headers_mut();
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type::served_content_type(&att.mime_type)),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition_header(inline, &att.filename))
            .map_err(ApiError::internal)?,
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private"));
    Ok(res)
}

async fn signed_url(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(id): ApiPath<Id>,
) -> Result<Envelope<Value>, ApiError> {
    let att = attachments::get(&s.pool, user.id, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Attachment"))?;
    let expires = now_secs() + s.config.mail.download_ttl.as_secs();
    let d = if inline_ok(&att.mime_type) {
        Disposition::Inline
    } else {
        Disposition::Attachment
    };
    let token = s.store.signer.sign(&att.storage_key, d, expires);
    Ok(Envelope::ok(json!({
        "url": format!("/api/{}/attachments/file/{}?token={token}", s.config.api_version, att.id),
        "expiresAt": ts(chrono::DateTime::from_timestamp(expires as i64, 0).unwrap_or_default()),
    })))
}

/// Uploader or project admin.
async fn remove(
    State(s): State<AppState>,
    AuthUser(user): AuthUser,
    ApiPath(id): ApiPath<Id>,
) -> Result<Envelope<Value>, ApiError> {
    attachments::get(&s.pool, user.id, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Attachment"))?;
    let key = mailq::delete_attachment(&s.pool, user.id, id)
        .await?
        .ok_or_else(|| {
            ApiError::Forbidden("Only the uploader or a project admin can delete a file.".into())
        })?;
    s.store
        .fs()
        .await?
        .delete(&key)
        .await
        .map_err(ApiError::internal)?;
    Ok(Envelope::ok(json!({ "success": true, "id": id })))
}
