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
    let body: String = body.trim().chars().take(MAX_BODY_CHARS).collect();
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

async fn find_thread(
    state: &AppState,
    project_id: Id,
    email: &InboundEmail,
    from: &str,
    ticket_number: Option<i64>,
) -> sd_db::Result<Option<Id>> {
    let pool = &state.pool;
    // The webhook is signed, so its explicit ticket number is trusted.
    if let Some(n) = ticket_number
        && let Some(id) = tickets::find_by_number(pool, project_id, n).await?
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
        {
            return Ok(Some(t));
        }
    }
    if !ids.is_empty()
        && let Some(t) = mailq::find_ticket_by_refs(pool, project_id, &ids).await?
    {
        return Ok(Some(t));
    }
    // The subject tag is the weakest signal (anyone can type it): only the ticket's own client may
    // use it, so a stranger cannot inject messages into someone else's ticket.
    if let Some(n) = parse_ticket_number(&email.subject)
        && let Some(id) = tickets::find_by_number(pool, project_id, i64::from(n)).await?
        && let Some(t) = mailq::ticket_mail(pool, project_id, id).await?
        && t.client_email.eq_ignore_ascii_case(from)
    {
        return Ok(Some(id));
    }
    Ok(None)
}

async fn store_attachments(
    state: &AppState,
    project_id: Id,
    ticket_id: Id,
    message_id: Id,
    files: &[InboundAttachment],
) {
    let Ok(fs) = state.store.fs().await else {
        tracing::error!("storage unavailable; inbound attachments dropped");
        return;
    };
    for f in files {
        let filename = sanitize_filename(&f.filename);
        let mime = normalize(&f.content_type).unwrap_or_else(|| "application/octet-stream".into());
        let meta = PutMeta {
            original_filename: Some(filename.clone()),
            content_type: Some(mime.clone()),
        };
        let obj = match fs.put(&f.bytes[..], meta).await {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!(error = %e, "inbound attachment not stored");
                continue;
            }
        };
        let new = attachments::NewAttachment {
            filename,
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

    let body = body_of(email);
    let references = (!email.references.is_empty()).then(|| email.references.join(" "));
    let received = email.date.and_then(|t| DateTime::from_timestamp(t, 0));
    let email_to = (!email.to.is_empty()).then(|| email.to.join(", "));

    if let Some(ticket_id) = find_thread(state, project_id, email, &from, ticket_number).await? {
        let posted = messages::post_inbound(
            &state.pool,
            &NewInbound {
                project_id,
                ticket_id,
                sender_email: from,
                sender_name: email.from_name.clone(),
                body,
                html_body: None,
                email_message_id: Some(email.message_id.clone()),
                email_in_reply_to: email.in_reply_to.clone(),
                email_references: references,
                email_from: email.from.clone(),
                email_to,
                email_subject: Some(email.subject.clone()),
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
            client_name: email.from_name.clone().filter(|n| !n.trim().is_empty()),
            subject: clean_subject(&email.subject),
            body,
            email_message_id: email.message_id.clone(),
            email_in_reply_to: email.in_reply_to.clone(),
            email_references: references,
            email_from: email.from.clone(),
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
    fn subjects_are_cleaned_and_bounded() {
        assert_eq!(clean_subject("Re: Fwd: Hello\r\nBcc: x"), "Hello Bcc: x");
        assert_eq!(clean_subject("   "), "(no subject)");
        assert_eq!(clean_subject("Re:"), "(no subject)");
        assert_eq!(clean_subject(&"a".repeat(1000)).chars().count(), 255);
    }
}
