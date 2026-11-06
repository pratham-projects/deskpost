//! The one place inbound mail (IMAP poll and the signed webhook) becomes tickets and messages.
//!
//! Order: refuse machine mail and our own mail, find the ticket it belongs to (explicit number,
//! `In-Reply-To`/`References`, then the `[Ticket #N]` subject tag), otherwise open a new ticket.
//! The Message-ID unique key makes replays a no-op even when the webhook and IMAP race.

use chrono::DateTime;
use sd_core::body::{body_from_parts, html_to_text};
use sd_core::content_type::normalize;
use sd_core::email_addr::{is_bounce_message, normalize_email};
use sd_core::filename::sanitize_filename;
use sd_core::threading::{parse_ticket_number, strip_reply_prefixes, ticket_id_from_message_id};
use sd_db::mailq::{self, NewInboundTicket};
use sd_db::messages::NewInbound;
use sd_db::{EmailResponseStatus, Id, attachments, messages, projects, tickets};
use sd_mail::{InboundAttachment, InboundEmail, Limits};
use sd_storage::{PutMeta, Storage};

use crate::config::Config;
use crate::mail_out::enqueue_auto_response;
use crate::state::AppState;

const MAX_BODY_CHARS: usize = 50_000;
const MAX_SUBJECT_CHARS: usize = 255;
const MAX_NAME_CHARS: usize = 255;
/// RFC 5322 line limit; caps every stored header-ish column.
const MAX_HEADER_CHARS: usize = 998;
const MAX_REFERENCES_CHARS: usize = 4000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Created {
        ticket_id: Id,
        ticket_number: i64,
        message_id: Id,
    },
    Replied {
        ticket_id: Id,
        message_id: Id,
    },
    /// This Message-ID was stored before.
    Duplicate,
    /// Not mail to turn into a ticket; the reason is for logs and tests.
    Ignored(&'static str),
}

/// Parser caps for untrusted mail, from the configured upload limits.
pub fn inbound_limits(cfg: &Config) -> Limits {
    Limits {
        max_message_bytes: cfg.mail.max_request_bytes,
        max_attachment_bytes: cfg.mail.max_file_bytes,
        max_attachments: cfg.mail.max_files,
        ..Limits::default()
    }
}

/// Strip NUL and control characters (Postgres `text` rejects NUL) and cap the length, so no inbound
/// value can violate a column constraint and wedge the mailbox. `keep_newlines` is for bodies.
pub(crate) fn clean_text(raw: &str, max_chars: usize, keep_newlines: bool) -> String {
    raw.chars()
        .map(|c| match c {
            '\n' | '\r' | '\t' if keep_newlines => c,
            c if c.is_control() => ' ',
            c => c,
        })
        .take(max_chars)
        .collect()
}

fn opt_line(raw: &Option<String>, max_chars: usize) -> Option<String> {
    raw.as_deref()
        .map(|v| clean_text(v, max_chars, false))
        .filter(|v| !v.trim().is_empty())
}

fn clean_subject(raw: &str) -> String {
    let s: String = strip_reply_prefixes(raw)
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s: String = s.chars().take(MAX_SUBJECT_CHARS).collect();
    if s.is_empty() {
        "(no subject)".into()
    } else {
        s
    }
}

/// Text of the mail without quotes and signatures. A forward whose own text is empty keeps the
/// forwarded text instead of becoming an empty message.
fn body_of(email: &InboundEmail) -> String {
    let mut body = body_from_parts(email.text.as_deref(), email.html.as_deref());
    if body.trim().is_empty() {
        body = match (email.text.as_deref(), email.html.as_deref()) {
            (Some(t), _) if !t.trim().is_empty() => t.to_string(),
            (_, Some(h)) => html_to_text(h),
            _ => String::new(),
        };
    }
    let body = clean_text(body.trim(), MAX_BODY_CHARS, true);
    let body = body.trim().to_string();
    if body.is_empty() {
        if email.attachments.is_empty() {
            "(empty message)".into()
        } else {
            "(attachment only)".into()
        }
    } else {
        body
    }
}

/// The ticket this mail continues, if any. Every signal (explicit number, Message-ID we minted,
/// `References`, subject tag) is only honoured when the sender is the ticket's client or an address
/// that already wrote on it; otherwise the next signal is tried and finally a new ticket opens.
async fn find_thread(
    state: &AppState,
    project_id: Id,
    email: &InboundEmail,
    from: &str,
    ticket_number: Option<i64>,
) -> sd_db::Result<Option<Id>> {
    let pool = &state.pool;
    let allowed = |t: Id| mailq::sender_on_ticket(pool, project_id, t, from);
    if let Some(n) = ticket_number
        && let Some(id) = tickets::find_by_number(pool, project_id, n).await?
        && allowed(id).await?
    {
        return Ok(Some(id));
    }
    let ids: Vec<String> = email
        .in_reply_to
        .iter()
        .chain(email.references.iter())
        .cloned()
        .collect();
    // Message-IDs we minted name the ticket directly (the project check keeps it per tenant).
    for id in &ids {
        if let Some(t) = ticket_id_from_message_id(&format!("<{id}>"))
            && let Ok(t) = t.parse::<Id>()
            && mailq::ticket_in_project(pool, project_id, t).await?
            && allowed(t).await?
        {
            return Ok(Some(t));
        }
    }
    if !ids.is_empty()
        && let Some(t) = mailq::find_ticket_by_refs(pool, project_id, &ids).await?
        && allowed(t).await?
    {
        return Ok(Some(t));
    }
    if let Some(n) = parse_ticket_number(&email.subject)
        && let Some(id) = tickets::find_by_number(pool, project_id, i64::from(n)).await?
        && allowed(id).await?
    {
        return Ok(Some(id));
    }
    Ok(None)
}

/// Keep the mail's files. Files that cannot be kept (storage error, quota) are named in a note on
/// the message so the loss is visible instead of silent.
async fn store_attachments(
    state: &AppState,
    project_id: Id,
    ticket_id: Id,
    message_id: Id,
    files: &[InboundAttachment],
) {
    if files.is_empty() {
        return;
    }
    let mut dropped: Vec<String> = Vec::new();
    match state.store.fs().await {
        Err(e) => {
            tracing::error!(error = ?e, "storage unavailable; inbound attachments dropped");
            dropped.extend(files.iter().map(|f| sanitize_filename(&f.filename)));
        }
        Ok(fs) => {
            for f in files {
                let filename = sanitize_filename(&f.filename);
                let mime =
                    normalize(&f.content_type).unwrap_or_else(|| "application/octet-stream".into());
                let meta = PutMeta {
                    original_filename: Some(filename.clone()),
                    content_type: Some(mime.clone()),
                };
                let obj = match fs.put(&f.bytes[..], meta).await {
                    Ok(o) => o,
                    Err(e) => {
                        tracing::warn!(error = %e, "inbound attachment not stored");
                        dropped.push(filename);
                        continue;
                    }
                };
                let new = attachments::NewAttachment {
                    filename: filename.clone(),
                    mime_type: mime,
                    size_bytes: obj.size as i64,
                    storage_key: obj.key.clone(),
                    message_id: Some(message_id),
                };
                let kept = matches!(
                    attachments::create_for_project(&state.pool, project_id, ticket_id, &new).await,
                    Ok(attachments::Stored::Ok(_))
                );
                if !kept {
                    tracing::warn!(%ticket_id, "inbound attachment rejected (quota or database)");
                    let _ = fs.delete(&obj.key).await;
                    dropped.push(filename);
                }
            }
        }
    }
    if !dropped.is_empty() {
        let note = format!(
            "[{} attachment(s) could not be stored: {}]",
            dropped.len(),
            dropped.join(", ")
        );
        if let Err(e) = messages::append_body(&state.pool, project_id, message_id, &note).await {
            tracing::warn!(error = %e, "attachment note not saved");
        }
    }
}

/// Turn one inbound email into a new ticket or a reply. `ticket_number` is the webhook's explicit hint.
pub async fn ingest(
    state: &AppState,
    project_id: Id,
    email: &InboundEmail,
    ticket_number: Option<i64>,
) -> sd_db::Result<Outcome> {
    let Some(settings) = projects::mail_settings(&state.pool, project_id).await? else {
        return Ok(Outcome::Ignored("unknown project"));
    };
    let Some(from) = email.from.as_deref().and_then(normalize_email) else {
        return Ok(Outcome::Ignored("no valid sender"));
    };
    if email.auto_generated {
        return Ok(Outcome::Ignored("auto-generated mail"));
    }
    if is_bounce_message(&from) {
        return Ok(Outcome::Ignored("system or no-reply sender"));
    }
    if settings
        .smtp_email
        .as_deref()
        .is_some_and(|own| own.eq_ignore_ascii_case(&from))
    {
        return Ok(Outcome::Ignored("sent from the project's own mailbox"));
    }
    if ticket_id_from_message_id(&format!("<{}>", email.message_id)).is_some() {
        return Ok(Outcome::Ignored("our own Message-ID"));
    }

    // Files need the store; if it is down, fail before anything is committed so the poller does not
    // advance its cursor and the webhook provider retries, instead of losing the files for good.
    if !email.attachments.is_empty()
        && let Err(e) = state.store.fs().await
    {
        return Err(sd_db::Error::Io(std::io::Error::other(format!(
            "attachment storage unavailable: {e:?}"
        ))));
    }
    let body = body_of(email);
    let references = (!email.references.is_empty())
        .then(|| clean_text(&email.references.join(" "), MAX_REFERENCES_CHARS, false));
    let in_reply_to = opt_line(&email.in_reply_to, MAX_HEADER_CHARS);
    let message_id = clean_text(&email.message_id, MAX_HEADER_CHARS, false);
    let email_from = opt_line(&email.from, MAX_HEADER_CHARS);
    let from_name = opt_line(&email.from_name, MAX_NAME_CHARS);
    let received = email.date.and_then(|t| DateTime::from_timestamp(t, 0));
    let email_to =
        (!email.to.is_empty()).then(|| clean_text(&email.to.join(", "), MAX_HEADER_CHARS, false));

    if let Some(ticket_id) = find_thread(state, project_id, email, &from, ticket_number).await? {
        let posted = messages::post_inbound(
            &state.pool,
            &NewInbound {
                project_id,
                ticket_id,
                sender_email: from,
                sender_name: from_name.clone(),
                body,
                html_body: None,
                email_message_id: Some(message_id.clone()),
                email_in_reply_to: in_reply_to.clone(),
                email_references: references,
                email_from: email_from.clone(),
                email_to,
                email_subject: Some(clean_text(&email.subject, MAX_HEADER_CHARS, false)),
                email_received_at: received,
            },
        )
        .await?;
        let Some(m) = posted else {
            return Ok(Outcome::Duplicate);
        };
        store_attachments(state, project_id, ticket_id, m.id, &email.attachments).await;
        return Ok(Outcome::Replied {
            ticket_id,
            message_id: m.id,
        });
    }

    let created = mailq::create_inbound_ticket(
        &state.pool,
        &NewInboundTicket {
            project_id,
            client_email: from,
            client_name: from_name,
            subject: clean_subject(&email.subject),
            body,
            email_message_id: message_id,
            email_in_reply_to: in_reply_to,
            email_references: references,
            email_from,
            email_to,
            email_received_at: received,
        },
    )
    .await?;
    let Some(c) = created else {
        return Ok(Outcome::Duplicate);
    };
    store_attachments(
        state,
        project_id,
        c.ticket_id,
        c.message_id,
        &email.attachments,
    )
    .await;
    if let Err(e) = enqueue_auto_response(
        &state.pool,
        project_id,
        c.ticket_id,
        EmailResponseStatus::Created,
    )
    .await
    {
        tracing::warn!(error = %e, "auto-response not queued");
    }
    Ok(Outcome::Created {
        ticket_id: c.ticket_id,
        ticket_number: c.ticket_number,
        message_id: c.message_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_text_cannot_violate_column_rules() {
        assert_eq!(clean_text("a\0b\u{7}c", 10, false), "a b c");
        assert_eq!(clean_text("a\nb", 10, true), "a\nb");
        assert_eq!(clean_text("a\nb", 10, false), "a b");
        assert_eq!(
            clean_text(&"x".repeat(300), 255, false).chars().count(),
            255
        );
        assert!(!clean_text("a\0", 10, true).contains('\0'));
    }

    #[test]
    fn subjects_are_cleaned_and_bounded() {
        assert_eq!(clean_subject("Re: Fwd: Hello\r\nBcc: x"), "Hello Bcc: x");
        assert_eq!(clean_subject("   "), "(no subject)");
        assert_eq!(clean_subject("Re:"), "(no subject)");
        assert_eq!(clean_subject(&"a".repeat(1000)).chars().count(), 255);
    }
}
