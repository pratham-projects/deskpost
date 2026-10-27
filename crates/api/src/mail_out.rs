//! Outbound mail: agent replies, auto-responses and the persisted retry queue.
//!
//! `OutboundMailer` is the seam the ticket routes use; production sends over the project's SMTP
//! settings, tests swap in `Mailer::Recording`. (R22 may define the same trait; the merge keeps one.)

use std::future::Future;
use std::sync::{Arc, Mutex};

use sd_core::secret::SecretBox;
use sd_core::threading::{
    build_references, parse_ticket_number, reply_message_id, ticket_message_id, ticket_subject,
};
use sd_db::projects::MailSettings;
use sd_db::{EmailResponseStatus, Id, Pool, email_logs, email_responses, mailq, projects};
use sd_mail::{OutAttachment, OutboundEmail, SmtpConfig, TlsMode};
use sd_storage::Storage;
use tokio::io::AsyncReadExt;

use crate::config::Config;
use crate::error::{ApiError, Fields};
use crate::state::AppState;
use crate::store::Store;

/// Gives up after this many sends.
pub const MAX_ATTEMPTS: i32 = 5;
const OUTBOX_BATCH: i64 = 20;
const LEASE_SECS: i64 = 300;
const ERROR_LEN: usize = 300;
const EXCERPT_CHARS: usize = 300;

/// AAD binding a sealed password to its project and field. `routes::projects` must seal with the same.
pub fn smtp_aad(project: Id) -> Vec<u8> {
    format!("{project}:smtp_password").into_bytes()
}

pub fn imap_aad(project: Id) -> Vec<u8> {
    format!("{project}:imap_password").into_bytes()
}

/// Seconds to wait after the `attempts`-th failed send: 30 s, 2 min, 8 min, 32 min, then 1 h.
pub fn backoff_secs(attempts: i32) -> i64 {
    let exp = (attempts.max(1) - 1).min(10) as u32;
    (30i64 * 4i64.pow(exp)).min(3600)
}

/// Port-based TLS choice: 465/993 implicit, everything else STARTTLS; plaintext only when configured.
pub fn tls_for(port: u16, plaintext: bool, implicit_port: u16) -> TlsMode {
    if plaintext {
        TlsMode::None
    } else if port == implicit_port {
        TlsMode::Implicit
    } else {
        TlsMode::StartTls
    }
}

/// Error text safe to store and show: secrets removed, one line, bounded.
pub fn redact(msg: &str, secrets: &[&str]) -> String {
    let mut out = msg.to_string();
    for s in secrets.iter().filter(|s| !s.is_empty()) {
        out = out.replace(s, "***");
    }
    let one_line: String = out
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    one_line.chars().take(ERROR_LEN).collect()
}

fn bare(id: &str) -> String {
    id.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_string()
}

#[derive(Debug, Clone)]
pub struct ReplyMail {
    pub project_id: Id,
    pub ticket_id: Id,
    pub sent_by: Option<Id>,
    pub body: String,
    /// Attachments of the same ticket to put on the mail.
    pub attachment_ids: Vec<Id>,
}

/// What to store on the message row so the thread continues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentReply {
    /// Without angle brackets.
    pub email_message_id: String,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
    pub to: String,
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailError {
    /// The project has no usable SMTP settings (API: 422).
    NotConfigured,
    /// The server refused or was unreachable (API: 502).
    Delivery(String),
    Internal(String),
}

impl From<MailError> for ApiError {
    fn from(e: MailError) -> Self {
        match e {
            MailError::NotConfigured => {
                let mut f = Fields::default();
                f.add("smtp", "SMTP not configured");
                f.finish().unwrap_err()
            }
            MailError::Delivery(e) => {
                tracing::warn!(error = %e, "mail delivery failed");
                ApiError::MailDeliveryFailed
            }
            MailError::Internal(e) => ApiError::internal(e),
        }
    }
}

impl From<sd_db::Error> for MailError {
    fn from(e: sd_db::Error) -> Self {
        Self::Internal(e.to_string())
    }
}

pub trait OutboundMailer: Send + Sync {
    /// Email an agent reply to the ticket's client. Sent before the message is saved: on `Err` the
    /// caller saves nothing.
    fn send_reply(
        &self,
        mail: ReplyMail,
    ) -> impl Future<Output = Result<SentReply, MailError>> + Send;
}

#[derive(Clone)]
pub enum Mailer {
    Smtp(SmtpMailer),
    Recording(Arc<RecordingMailer>),
}

impl Mailer {
    pub fn smtp(
        pool: &Pool,
        secrets: &Arc<SecretBox>,
        config: &Arc<Config>,
        store: &Arc<Store>,
    ) -> Self {
        Self::Smtp(SmtpMailer {
            pool: pool.clone(),
            secrets: secrets.clone(),
            config: config.clone(),
            store: store.clone(),
        })
    }
}

impl OutboundMailer for Mailer {
    async fn send_reply(&self, mail: ReplyMail) -> Result<SentReply, MailError> {
        match self {
            Self::Smtp(m) => m.send_reply(mail).await,
            Self::Recording(m) => m.send_reply(mail).await,
        }
    }
}

/// Test double: records every request and answers with a made-up Message-ID, or with the error
/// queued by `fail_next`.
#[derive(Default)]
pub struct RecordingMailer {
    pub sent: Mutex<Vec<ReplyMail>>,
    pub fail_next: Mutex<Option<MailError>>,
}

impl OutboundMailer for RecordingMailer {
    async fn send_reply(&self, mail: ReplyMail) -> Result<SentReply, MailError> {
        if let Some(e) = self.fail_next.lock().unwrap().take() {
            return Err(e);
        }
        let n = {
            let mut sent = self.sent.lock().unwrap();
            sent.push(mail);
            sent.len()
        };
        Ok(SentReply {
            email_message_id: format!("recorded-{n}@test.invalid"),
            in_reply_to: None,
            references: None,
            to: "client@test.invalid".into(),
            subject: "recorded".into(),
        })
    }
}

#[derive(Clone)]
pub struct SmtpMailer {
    pool: Pool,
    secrets: Arc<SecretBox>,
    config: Arc<Config>,
    store: Arc<Store>,
}

/// SMTP settings of a project with the password opened. Also returns the plaintext password so the
/// caller can redact it from error text.
fn smtp_config(
    s: &MailSettings,
    secrets: &SecretBox,
    plaintext: bool,
) -> Result<(SmtpConfig, String), MailError> {
    let (Some(host), Some(from)) = (s.smtp_host.as_deref(), s.smtp_email.as_deref()) else {
        return Err(MailError::NotConfigured);
    };
    let port = u16::try_from(s.smtp_port).map_err(|_| MailError::NotConfigured)?;
    let password = match &s.smtp_password_encrypted {
        Some(sealed) => secrets
            .open(sealed, &smtp_aad(s.project_id))
            .map_err(|_| MailError::Internal("cannot open the stored SMTP password".into()))?,
        None => String::new(),
    };
    let mut cfg = SmtpConfig::new(host, port, tls_for(port, plaintext, 465), from);
    cfg.from_name = Some(s.email_from_name.clone());
    if let Some(user) = s.smtp_username.as_deref().filter(|u| !u.is_empty()) {
        cfg.username = Some(user.to_string());
        cfg.password = Some(password.clone());
    }
    Ok((cfg, password))
}

fn domain_of(addr: &str) -> &str {
    addr.rsplit('@').next().unwrap_or("localhost")
}

impl SmtpMailer {
    async fn attachments(&self, mail: &ReplyMail) -> Result<Vec<OutAttachment>, MailError> {
        if mail.attachment_ids.is_empty() {
            return Ok(vec![]);
        }
        let rows = mailq::attachments_of_ticket(
            &self.pool,
            mail.project_id,
            mail.ticket_id,
            &mail.attachment_ids,
        )
        .await?;
        if rows.len() != mail.attachment_ids.len() {
            return Err(MailError::Internal(
                "attachment does not belong to the ticket".into(),
            ));
        }
        let fs = self
            .store
            .fs()
            .await
            .map_err(|_| MailError::Internal("storage unavailable".into()))?;
        let mut out = Vec::with_capacity(rows.len());
        for a in rows {
            let (reader, _) = fs
                .get(&a.storage_key)
                .await
                .map_err(|e| MailError::Internal(e.to_string()))?;
            let mut bytes = Vec::new();
            reader
                .take(a.size_bytes as u64 + 1)
                .read_to_end(&mut bytes)
                .await
                .map_err(|e| MailError::Internal(e.to_string()))?;
            out.push(OutAttachment {
                filename: a.filename,
                content_type: a.mime_type,
                bytes,
            });
        }
        Ok(out)
    }
}

impl OutboundMailer for SmtpMailer {
    async fn send_reply(&self, mail: ReplyMail) -> Result<SentReply, MailError> {
        let settings = projects::mail_settings(&self.pool, mail.project_id)
            .await?
            .ok_or(MailError::NotConfigured)?;
        let (smtp, password) = smtp_config(&settings, &self.secrets, self.config.mail.plaintext)?;
        let ticket = mailq::ticket_mail(&self.pool, mail.project_id, mail.ticket_id)
            .await?
            .ok_or_else(|| MailError::Internal("ticket not found".into()))?;
        let thread = mailq::thread_ids(&self.pool, mail.project_id, mail.ticket_id).await?;
        let (in_reply_to, references) = match thread.split_last() {
            Some((parent, previous)) => (Some(parent.clone()), build_references(previous, parent)),
            None => (None, vec![]),
        };
        let domain = domain_of(&smtp.from_address).to_string();
        let message_id = reply_message_id(
            &mail.ticket_id.to_string(),
            &uuid::Uuid::new_v4().to_string(),
            &domain,
        )
        .map(|m| bare(&m));
        let subject = ticket_subject(&ticket.project_name, ticket.ticket_number as u32, true);
        let attachments = self.attachments(&mail).await?;
        let email = OutboundEmail {
            to: vec![ticket.client_email.clone()],
            subject: subject.clone(),
            text: Some(mail.body.clone()),
            message_id: message_id.clone(),
            in_reply_to: in_reply_to.clone(),
            references: references.clone(),
            attachments,
            ..Default::default()
        };
        let log = email_logs::create(
            &self.pool,
            &email_logs::NewEmailLog {
                project_id: mail.project_id,
                ticket_id: mail.ticket_id,
                message_id: None,
                to_email: ticket.client_email.clone(),
                subject: Some(subject.clone()),
                sent_by_user_id: mail.sent_by,
            },
        )
        .await?;
        match sd_mail::send(&smtp, &email).await {
            Ok(id) => {
                let id = bare(&id);
                email_logs::mark_sent(&self.pool, log.id, &id).await?;
                Ok(SentReply {
                    email_message_id: id,
                    in_reply_to,
                    references: (!references.is_empty()).then(|| references.join(" ")),
                    to: ticket.client_email,
                    subject,
                })
            }
            Err(e) => {
                let msg = redact(&e.to_string(), &[&password]);
                email_logs::mark_failed(&self.pool, log.id, &msg).await?;
                Err(MailError::Delivery(msg))
            }
        }
    }
}

// ---- auto-responses ----

fn ctl_free(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// Subject of an auto-response: the project's own text with the ticket tag guaranteed (replies are
/// threaded by it), or the standard subject when none is configured.
fn auto_subject(custom: &str, project: &str, number: i64) -> String {
    let custom = ctl_free(custom);
    let custom = custom.trim();
    if custom.is_empty() {
        ticket_subject(project, number as u32, false)
    } else if parse_ticket_number(custom) == Some(number as u32) {
        custom.to_string()
    } else {
        format!("{custom} [Ticket #{number}]")
    }
}

/// Queue the project's enabled auto-response for a ticket event. `Ok(false)` when there is nothing
/// to send (no enabled response, or a flood bound was hit). Call after the ticket change committed.
pub async fn enqueue_auto_response(
    pool: &Pool,
    project_id: Id,
    ticket_id: Id,
    kind: EmailResponseStatus,
) -> sd_db::Result<bool> {
    let Some(resp) = email_responses::get_enabled(pool, project_id, kind).await? else {
        return Ok(false);
    };
    let Some(t) = mailq::ticket_mail(pool, project_id, ticket_id).await? else {
        return Ok(false);
    };
    let subject = auto_subject(&resp.subject, &t.project_name, t.ticket_number);
    Ok(mailq::enqueue(
        pool,
        project_id,
        ticket_id,
        kind.as_str(),
        &t.client_email,
        &subject,
    )
    .await?
    .is_some())
}

/// Send what is due. Each job gets one attempt; failures are rescheduled with backoff or, after
/// `MAX_ATTEMPTS`, given up. Returns the number of jobs handled. Never more than one batch.
pub async fn run_outbox_once(state: &AppState) -> sd_db::Result<usize> {
    let jobs = mailq::claim_due(&state.pool, OUTBOX_BATCH, LEASE_SECS).await?;
    let n = jobs.len();
    for job in jobs {
        let outcome = send_job(state, &job).await;
        match outcome {
            Ok(id) => mailq::mark_sent(&state.pool, &job, &id).await?,
            Err((msg, retryable)) => {
                let retry =
                    (retryable && job.attempts < MAX_ATTEMPTS).then(|| backoff_secs(job.attempts));
                mailq::mark_failed(&state.pool, &job, &msg, retry).await?;
            }
        }
    }
    Ok(n)
}

/// `Err((message, retryable))`.
async fn send_job(state: &AppState, job: &mailq::OutboxJob) -> Result<String, (String, bool)> {
    let fatal = |m: &str| (m.to_string(), false);
    let db = |e: sd_db::Error| (redact(&e.to_string(), &[]), true);
    let settings = projects::mail_settings(&state.pool, job.project_id)
        .await
        .map_err(db)?
        .ok_or_else(|| fatal("project not found"))?;
    let (smtp, password) = smtp_config(&settings, &state.secrets, state.config.mail.plaintext)
        .map_err(|_| fatal("SMTP not configured"))?;
    let kind = job
        .kind
        .parse::<EmailResponseStatus>()
        .map_err(|_| fatal("unknown response kind"))?;
    let resp = email_responses::get_enabled(&state.pool, job.project_id, kind)
        .await
        .map_err(db)?
        .ok_or_else(|| fatal("auto-response disabled or removed"))?;
    let t = mailq::ticket_mail(&state.pool, job.project_id, job.ticket_id)
        .await
        .map_err(db)?
        .ok_or_else(|| fatal("ticket not found"))?;
    let domain = domain_of(&smtp.from_address).to_string();
    let ticket_id = job.ticket_id.to_string();
    let (message_id, in_reply_to, references) = match kind {
        EmailResponseStatus::Created => (ticket_message_id(&ticket_id, &domain), None, vec![]),
        EmailResponseStatus::Resolved => {
            let thread = mailq::thread_ids(&state.pool, job.project_id, job.ticket_id)
                .await
                .map_err(db)?;
            let (parent, refs) = match thread.split_last() {
                Some((p, prev)) => (Some(p.clone()), build_references(prev, p)),
                None => (None, vec![]),
            };
            let mid = reply_message_id(&ticket_id, &uuid::Uuid::new_v4().to_string(), &domain);
            (mid, parent, refs)
        }
    };
    let mut text = resp.body.clone();
    if let Some(d) = t.description.as_deref().filter(|d| !d.trim().is_empty()) {
        // Only a short excerpt: the address may be spoofed, and we must not reflect a whole mail at it.
        text.push_str("\n\n---\nYour message:\n");
        text.push_str(&d.chars().take(EXCERPT_CHARS).collect::<String>());
        if d.chars().count() > EXCERPT_CHARS {
            text.push('…');
        }
    }
    let email = OutboundEmail {
        to: vec![t.client_email.clone()],
        subject: auto_subject(&resp.subject, &t.project_name, t.ticket_number),
        text: Some(text),
        message_id: message_id.map(|m| bare(&m)),
        in_reply_to,
        references,
        ..Default::default()
    };
    sd_mail::send(&smtp, &email)
        .await
        .map(|id| bare(&id))
        .map_err(|e| (redact(&e.to_string(), &[&password]), true))
}
