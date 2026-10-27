//! `POST /incoming-email`: signed webhook for providers that push mail. Feeds the same `ingest`
//! as the IMAP poller.

use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::HeaderMap;
use axum::routing::post;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, KeyInit, Mac};
use sd_core::email_addr::normalize_email;
use sd_core::threading::parse_message_ids;
use sd_db::projects;
use sd_mail::{InboundAttachment, InboundEmail};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::client_ip::ClientIp;
use crate::error::{ApiError, Envelope, Fields};
use crate::ingest::{Outcome, ingest};
use crate::ratelimit::Rule;
use crate::state::AppState;

/// Allowed distance between the signed timestamp and our clock.
pub const TOLERANCE_SECS: i64 = 300;

pub fn router(body_limit: usize) -> Router<AppState> {
    Router::new().route(
        "/incoming-email",
        post(receive).layer(DefaultBodyLimit::max(body_limit)),
    )
}

/// Hex HMAC-SHA256 of `"{timestamp}.{body}"`, as providers must send in `X-Webhook-Signature`.
pub fn sign(secret: &[u8], timestamp: i64, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("hmac takes any key length");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Constant-time check of signature and freshness.
fn verify(secret: &[u8], headers: &HeaderMap, body: &[u8], now: i64) -> Result<(), ApiError> {
    let get = |n: &str| headers.get(n).and_then(|v| v.to_str().ok());
    let ts: i64 = get("x-webhook-timestamp")
        .and_then(|t| t.trim().parse().ok())
        .ok_or(ApiError::TokenInvalid)?;
    let sig = get("x-webhook-signature")
        .and_then(|s| unhex(s.trim()))
        .ok_or(ApiError::TokenInvalid)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("hmac takes any key length");
    mac.update(ts.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    // Signature first, so timing never tells whether a stale timestamp was otherwise valid.
    mac.verify_slice(&sig).map_err(|_| ApiError::TokenInvalid)?;
    if (now - ts).abs() > TOLERANCE_SECS {
        return Err(ApiError::TokenInvalid);
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Payload {
    from: String,
    to: String,
    #[serde(default)]
    subject: String,
    body: Option<String>,
    body_html: Option<String>,
    ticket_number: Option<i64>,
    message_id: Option<String>,
    in_reply_to: Option<String>,
    references: Option<String>,
    auto_submitted: Option<String>,
    precedence: Option<String>,
    #[serde(default)]
    attachments: Vec<PayloadFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PayloadFile {
    filename: Option<String>,
    content_type: Option<String>,
    content_base64: String,
}

fn bare(id: &str) -> String {
    id.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
        .to_string()
}

fn to_email(p: Payload, s: &AppState) -> Result<InboundEmail, ApiError> {
    let cfg = &s.config.mail;
    let mut f = Fields::default();
    if p.attachments.len() > cfg.max_files {
        return Err(ApiError::PayloadTooLarge);
    }
    let mut total = 0usize;
    let mut attachments = Vec::new();
    for a in &p.attachments {
        // Decoded size is at most 3/4 of the text, so the cap is checked before decoding.
        let approx = a.content_base64.len() / 4 * 3;
        total += approx;
        if approx > cfg.max_file_bytes + 3
            || total > cfg.max_request_bytes + 3 * p.attachments.len()
        {
            return Err(ApiError::PayloadTooLarge);
        }
        let Ok(bytes) = STANDARD.decode(a.content_base64.trim()) else {
            f.add("attachments", "contentBase64 must be base64.");
            continue;
        };
        attachments.push(InboundAttachment {
            filename: a.filename.clone().unwrap_or_else(|| "attachment".into()),
            content_type: a
                .content_type
                .clone()
                .unwrap_or_else(|| "application/octet-stream".into()),
            content_id: None,
            inline: false,
            bytes,
        });
    }
    let from = normalize_email(&p.from);
    if from.is_none() {
        f.add("from", "A valid sender address is required.");
    }
    if p.body.as_deref().is_none_or(str::is_empty)
        && p.body_html.as_deref().is_none_or(str::is_empty)
        && attachments.is_empty()
    {
        f.add("body", "Provide body, bodyHtml or attachments.");
    }
    f.finish()?;
    // Providers without a Message-ID still dedupe: the id is a digest of what identifies the mail.
    let message_id = match p.message_id.as_deref().map(bare).filter(|m| !m.is_empty()) {
        Some(m) => m,
        None => {
            let mut h = Sha256::new();
            for part in [
                &p.from,
                &p.to,
                &p.subject,
                p.body.as_deref().unwrap_or_default(),
                p.body_html.as_deref().unwrap_or_default(),
            ] {
                h.update((part.len() as u64).to_be_bytes());
                h.update(part.as_bytes());
            }
            let hex: String = h
                .finalize()
                .iter()
                .take(16)
                .map(|b| format!("{b:02x}"))
                .collect();
            format!("{hex}@generated.invalid")
        }
    };
    let auto = p
        .auto_submitted
        .as_deref()
        .is_some_and(|v| !v.trim().eq_ignore_ascii_case("no"))
        || p.precedence.as_deref().is_some_and(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "bulk" | "junk" | "list" | "auto_reply"
            )
        });
    Ok(InboundEmail {
        message_id,
        in_reply_to: p
            .in_reply_to
            .as_deref()
            .and_then(|h| parse_message_ids(h).into_iter().next())
            .map(|m| bare(&m)),
        references: p
            .references
            .as_deref()
            .map(|h| parse_message_ids(h).iter().map(|m| bare(m)).collect())
            .unwrap_or_default(),
        from,
        from_name: None,
        to: vec![p.to],
        subject: p.subject,
        text: p.body,
        html: p.body_html,
        attachments,
        date: None,
        auto_generated: auto,
    })
}

async fn receive(
    State(s): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Envelope<Value>, ApiError> {
    s.limiter.check(Rule::Webhook, ip)?;
    let Some(secret) = s.config.mail.webhook_secret.as_deref() else {
        return Err(ApiError::NotFound("Route not found.".into()));
    };
    let body = body.map_err(|_| ApiError::PayloadTooLarge)?;
    verify(secret, &headers, &body, chrono::Utc::now().timestamp())?;
    let payload: Payload = serde_json::from_slice(&body).map_err(|_| {
        let mut f = Fields::default();
        f.add("body", "Missing required fields or wrong types.");
        f.finish().unwrap_err()
    })?;
    let to = normalize_email(&payload.to).ok_or_else(|| {
        let mut f = Fields::default();
        f.add("to", "A valid recipient address is required.");
        f.finish().unwrap_err()
    })?;
    let project = projects::find_by_smtp_email(&s.pool, &to)
        .await?
        .ok_or_else(|| ApiError::not_found("Mailbox"))?;
    let number = payload.ticket_number;
    let email = to_email(payload, &s)?;
    match ingest(&s, project, &email, number).await? {
        Outcome::Created {
            ticket_id,
            message_id,
            ..
        } => Ok(Envelope::created(
            json!({ "ticketId": ticket_id, "messageId": message_id, "created": true }),
        )),
        Outcome::Replied {
            ticket_id,
            message_id,
        } => Ok(Envelope::created(
            json!({ "ticketId": ticket_id, "messageId": message_id, "created": false }),
        )),
        // Replays and machine mail are acknowledged so providers stop retrying.
        Outcome::Duplicate => Ok(Envelope::ok(json!({ "duplicate": true }))),
        Outcome::Ignored(reason) => Ok(Envelope::ok(json!({ "ignored": reason }))),
    }
}
