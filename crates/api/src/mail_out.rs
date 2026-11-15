//! Outbound mail: agent replies, auto-responses and the persisted retry queue.
//!
//! `OutboundMailer` is the seam the ticket routes use; production sends over the project's SMTP
//! settings, tests swap in `Mailer::Recording`. (R22 may define the same trait; the merge keeps one.)

use std::future::Future;
use std::sync::{Arc, Mutex};

use dp_core::secret::SecretBox;
use dp_core::threading::{
    build_references, parse_ticket_number, reply_message_id, ticket_message_id, ticket_subject,
};
use dp_db::projects::MailSettings;
use dp_db::{EmailResponseStatus, Id, Pool, email_responses, mailq, messages, projects};
use dp_mail::{OutAttachment, OutboundEmail, SmtpConfig, TlsMode};
use dp_storage::Storage;
use tokio::io::AsyncReadExt;

use crate::config::Config;
use crate::error::{ApiError, Fields};
use crate::state::AppState;
use crate::store::Store;

/// Gives up after this many sends.
pub const MAX_ATTEMPTS: i32 = 5;
const OUTBOX_BATCH: i64 = 20;
/// One send gets this long; a slow server must not hold the rest of the batch.
const JOB_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);
/// Lease of a claimed batch: every job may take `JOB_TIMEOUT`, so no later job's lease runs out
/// while an earlier one is still sending (a second instance would otherwise send it twice).
const LEASE_SECS: i64 = OUTBOX_BATCH * JOB_TIMEOUT.as_secs() as i64 + 60;
const ERROR_LEN: usize = 300;

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

impl From<dp_db::Error> for MailError {
    fn from(e: dp_db::Error) -> Self {
        Self::Internal(e.to_string())
    }
}

pub trait OutboundMailer: Send + Sync {
    /// Email an agent reply to the ticket's client. Called by the outbox worker for a saved message;
    /// on `Err` the job is retried or given up. The `email_logs` row belongs to the outbox job.
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

/// Account mail from the platform mailbox (`DP_SYSTEM_SMTP_*`), e.g. password reset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemMail {
    pub to: String,
    pub subject: String,
    pub text: String,
}

impl Mailer {
    /// Whether account mail can be sent at all (a system mailbox is configured, or a test double).
    pub fn system_enabled(&self) -> bool {
        match self {
            Self::Smtp(m) => m.config.mail.system_smtp.is_some(),
            Self::Recording(_) => true,
        }
    }

    pub async fn send_system(&self, mail: SystemMail) -> Result<(), MailError> {
        match self {
            Self::Smtp(m) => m.send_system(mail).await,
            Self::Recording(m) => {
                if let Some(e) = m.fail_next.lock().unwrap().take() {
                    return Err(e);
                }
                m.system_sent.lock().unwrap().push(mail);
                Ok(())
            }
        }
    }
}

impl SmtpMailer {
    async fn send_system(&self, mail: SystemMail) -> Result<(), MailError> {
        let Some(sys) = &self.config.mail.system_smtp else {
            return Err(MailError::NotConfigured);
        };
        let mut cfg = SmtpConfig::new(
            &sys.host,
            sys.port,
            tls_for(sys.port, self.config.mail.plaintext, 465),
            &sys.from,
        );
        cfg.allow_private = true;
        cfg.from_name = Some("Deskpost".into());
        if let (Some(u), Some(p)) = (&sys.username, &sys.password) {
            cfg.username = Some(u.clone());
            cfg.password = Some(p.clone());
        }
        let email = OutboundEmail {
            to: vec![mail.to],
            subject: mail.subject,
            text: Some(mail.text),
            ..Default::default()
        };
        let secrets = [sys.password.as_deref().unwrap_or("")];
        dp_mail::send(&cfg, &email)
            .await
            .map(|_| ())
            .map_err(|e| MailError::Delivery(redact(&e.to_string(), &secrets)))
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
    pub system_sent: Mutex<Vec<SystemMail>>,
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
    allow_private: bool,
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
    cfg.allow_private = allow_private;
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
        let total: i64 = rows.iter().map(|a| a.size_bytes.max(0)).sum();
        if total as usize > self.config.mail.max_request_bytes {
            return Err(MailError::Internal(
                "attachments are larger than the mail size limit".into(),
            ));
        }
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
        let (smtp, password) = smtp_config(
            &settings,
            &self.secrets,
            self.config.mail.plaintext,
            self.config.allow_private_mail_hosts,
        )?;
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
        let subject = ticket_subject(
            &ticket.project_name,
            u32::try_from(ticket.ticket_number).unwrap_or(u32::MAX),
            true,
        );
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
        match dp_mail::send(&smtp, &email).await {
            Ok(id) => Ok(SentReply {
                email_message_id: bare(&id),
                in_reply_to,
                references: (!references.is_empty()).then(|| references.join(" ")),
                to: ticket.client_email,
                subject,
            }),
            Err(e) => Err(MailError::Delivery(redact(&e.to_string(), &[&password]))),
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
    let n = u32::try_from(number).unwrap_or(u32::MAX);
    let custom = ctl_free(custom);
    let custom = custom.trim();
    if custom.is_empty() {
        ticket_subject(project, n, false)
    } else if parse_ticket_number(custom) == Some(n) {
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
) -> dp_db::Result<bool> {
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

/// Queue a "new ticket" notice for the members who asked for one (`shouldNotify`). Best effort: call
/// after the ticket committed and log, never fail, on error. Returns how many notices were queued.
pub async fn enqueue_team_notice(
    pool: &Pool,
    project_id: Id,
    ticket_id: Id,
) -> dp_db::Result<usize> {
    let Some(t) = mailq::ticket_mail(pool, project_id, ticket_id).await? else {
        return Ok(0);
    };
    let subject: String = format!("New ticket #{}: {}", t.ticket_number, ctl_free(&t.subject))
        .chars()
        .take(255)
        .collect();
    mailq::enqueue_team_notice(pool, project_id, ticket_id, &subject).await
}

/// Queue target for an agent reply: the ticket's client and the subject the mail will carry.
/// `None` when the ticket does not exist in the project.
pub async fn reply_outbox(
    pool: &Pool,
    project_id: Id,
    ticket_id: Id,
) -> dp_db::Result<Option<messages::ReplyOutbox>> {
    Ok(mailq::ticket_mail(pool, project_id, ticket_id)
        .await?
        .map(|t| messages::ReplyOutbox {
            subject: ticket_subject(
                &t.project_name,
                u32::try_from(t.ticket_number).unwrap_or(u32::MAX),
                true,
            ),
            to_email: t.client_email,
        }))
}

/// Send what is due. Each job gets one attempt, bounded by `JOB_TIMEOUT`; failures are rescheduled
/// with backoff or, after `MAX_ATTEMPTS`, given up. A bookkeeping error on one job never abandons
/// the rest of the batch. Returns the number of jobs handled. Never more than one batch.
pub async fn run_outbox_once(state: &AppState) -> dp_db::Result<usize> {
    let dead = mailq::dead_letter_exhausted(&state.pool, MAX_ATTEMPTS).await?;
    if dead > 0 {
        tracing::error!(dead, "outbox jobs given up after repeated worker crashes");
    }
    let jobs = mailq::claim_due(&state.pool, OUTBOX_BATCH, LEASE_SECS).await?;
    let n = jobs.len();
    for job in jobs {
        let outcome = match tokio::time::timeout(JOB_TIMEOUT, send_job(state, &job)).await {
            Ok(o) => o,
            Err(_) => Err(("send timed out".to_string(), true)),
        };
        let recorded = match outcome {
            Ok(id) => mailq::mark_sent(&state.pool, &job, &id).await,
            Err((msg, retryable)) => {
                let retry =
                    (retryable && job.attempts < MAX_ATTEMPTS).then(|| backoff_secs(job.attempts));
                mailq::mark_failed(&state.pool, &job, &msg, retry).await
            }
        };
        if let Err(e) = recorded {
            // The lease expires and the job comes back; the rest of the batch still goes out.
            tracing::error!(job = %job.id, error = %e, "could not record an outbox outcome");
        }
    }
    Ok(n)
}

/// An agent reply: rebuilt from the saved message, threaded, and its headers stored on success.
async fn send_reply_job(
    state: &AppState,
    job: &mailq::OutboxJob,
) -> Result<String, (String, bool)> {
    let db = |e: dp_db::Error| (redact(&e.to_string(), &[]), true);
    let message_id = job
        .message_id
        .ok_or_else(|| ("reply job without a message".to_string(), false))?;
    let (body, attachment_ids) = mailq::reply_source(&state.pool, message_id)
        .await
        .map_err(db)?
        .ok_or_else(|| ("message not found".to_string(), false))?;
    let mail = ReplyMail {
        project_id: job.project_id,
        ticket_id: job.ticket_id,
        sent_by: job.sent_by_user_id,
        body,
        attachment_ids,
    };
    match state.mailer.send_reply(mail).await {
        Ok(sent) => {
            if let Err(e) = messages::set_email_headers(
                &state.pool,
                message_id,
                &sent.email_message_id,
                sent.in_reply_to.as_deref(),
                sent.references.as_deref(),
                &sent.to,
                &sent.subject,
            )
            .await
            {
                tracing::error!(error = %e, "reply sent but mail headers not stored");
            }
            Ok(sent.email_message_id)
        }
        Err(MailError::NotConfigured) => Err(("SMTP not configured".to_string(), false)),
        Err(MailError::Delivery(e) | MailError::Internal(e)) => Err((redact(&e, &[]), true)),
    }
}

/// "New ticket" notice to one team member, from the project's own mailbox. Plain text with the
/// ticket number, sender and subject only; the description stays in the app.
async fn send_notice_job(
    state: &AppState,
    job: &mailq::OutboxJob,
) -> Result<String, (String, bool)> {
    let fatal = |m: &str| (m.to_string(), false);
    let db = |e: dp_db::Error| (redact(&e.to_string(), &[]), true);
    let settings = projects::mail_settings(&state.pool, job.project_id)
        .await
        .map_err(db)?
        .ok_or_else(|| fatal("project not found"))?;
    let (smtp, password) = smtp_config(
        &settings,
        &state.secrets,
        state.config.mail.plaintext,
        state.config.allow_private_mail_hosts,
    )
    .map_err(|_| fatal("SMTP not configured"))?;
    let to = mailq::log_recipient(&state.pool, job.email_log_id)
        .await
        .map_err(db)?
        .ok_or_else(|| fatal("email log not found"))?;
    let t = mailq::ticket_mail(&state.pool, job.project_id, job.ticket_id)
        .await
        .map_err(db)?
        .ok_or_else(|| fatal("ticket not found"))?;
    let email = OutboundEmail {
        to: vec![to],
        subject: format!(
            "[{}] New ticket #{}",
            ctl_free(&t.project_name),
            t.ticket_number
        ),
        text: Some(format!(
            "A new ticket was opened in {}.\n\nTicket: #{}\nFrom: {}\nSubject: {}\n",
            ctl_free(&t.project_name),
            t.ticket_number,
            ctl_free(&t.client_email),
            ctl_free(&t.subject),
        )),
        ..Default::default()
    };
    dp_mail::send(&smtp, &email)
        .await
        .map(|id| bare(&id))
        .map_err(|e| (redact(&e.to_string(), &[&password]), true))
}

/// `Err((message, retryable))`.
async fn send_job(state: &AppState, job: &mailq::OutboxJob) -> Result<String, (String, bool)> {
    if job.kind == "reply" {
        return send_reply_job(state, job).await;
    }
    if job.kind == "new_ticket" {
        return send_notice_job(state, job).await;
    }
    let fatal = |m: &str| (m.to_string(), false);
    let db = |e: dp_db::Error| (redact(&e.to_string(), &[]), true);
    let settings = projects::mail_settings(&state.pool, job.project_id)
        .await
        .map_err(db)?
        .ok_or_else(|| fatal("project not found"))?;
    let (smtp, password) = smtp_config(
        &settings,
        &state.secrets,
        state.config.mail.plaintext,
        state.config.allow_private_mail_hosts,
    )
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
    // Only what the project controls: the address may be spoofed, so no sender text is reflected.
    let text = resp.body.clone();
    let email = OutboundEmail {
        to: vec![t.client_email.clone()],
        subject: auto_subject(&resp.subject, &t.project_name, t.ticket_number),
        text: Some(text),
        message_id: message_id.map(|m| bare(&m)),
        in_reply_to,
        references,
        ..Default::default()
    };
    dp_mail::send(&smtp, &email)
        .await
        .map(|id| bare(&id))
        .map_err(|e| (redact(&e.to_string(), &[&password]), true))
}
