//! Per-project SMTP/IMAP settings: validation, write-only encrypted passwords, and the
//! connection tests (`docs/API.md` section 6).
//!
//! Passwords are sealed with [`password_aad`] (project id + field), which the mail worker must
//! pass to `SecretBox::open`. TLS is not stored: [`tls_for_port`] derives it from the port.

use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::routing::post;
use sd_core::email_addr::is_valid_email;
use sd_core::secret::SecretBox;
use sd_db::projects::{self, MailSettings, MailUpdate};
use sd_mail::{ImapConfig, ImapMailbox, SmtpConfig, TlsMode};
use serde::{Deserialize, Serialize};

use super::mail_guard;
use crate::client_ip::ClientIp;
use crate::error::{ApiError, Envelope, Fields};
use crate::extract::{ApiJson, ApiPath, AuthUser, GlobalAdmin, project_admin};
use crate::ratelimit::Rule;
use crate::state::AppState;

const MAX_PASSWORD_CHARS: usize = 512;
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Associated data binding a ciphertext to its project and field (`kind`: `smtp` or `imap`).
pub fn password_aad(project: sd_db::Id, kind: &str) -> Vec<u8> {
    format!("{project}:{kind}_password").into_bytes()
}

/// Implicit TLS on the well-known SSL ports, STARTTLS everywhere else.
pub fn tls_for_port(port: u16) -> TlsMode {
    if matches!(port, 465 | 993) {
        TlsMode::Implicit
    } else {
        TlsMode::StartTls
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/{id}/test-smtp", post(test_smtp))
        .route("/{id}/test-imap", post(test_imap))
        .route("/{id}/mailbox/approve", post(approve_mailbox))
}

/// Approve the project's mailbox address for inbound webhook routing. Global admins only: the
/// operator controls the mail provider, a tenant cannot prove an address by logging in to a server
/// of its own choosing or by receiving a code through it.
async fn approve_mailbox(
    State(s): State<AppState>,
    GlobalAdmin(_): GlobalAdmin,
    ApiPath(id): ApiPath<sd_db::Id>,
) -> Result<Envelope<serde_json::Value>, ApiError> {
    if !projects::approve_mailbox(&s.pool, id).await? {
        return Err(ApiError::not_found("Mailbox"));
    }
    Ok(Envelope::ok(serde_json::json!({ "approved": true })))
}

/// Mail part of the create/patch bodies. `None` = not sent = unchanged; `""` clears.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MailFields {
    pub reply_to_email: Option<String>,
    pub smtp_email: Option<String>,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<i64>,
    pub imap_host: Option<String>,
    pub imap_port: Option<i64>,
    pub imap_username: Option<String>,
    pub imap_password: Option<String>,
    pub imap_folder: Option<String>,
}

fn text(f: &mut Fields, field: &str, v: &mut Option<String>, max: usize) {
    if let Some(s) = v {
        *s = s.trim().to_string();
        if s.chars().count() > max || s.chars().any(char::is_control) {
            f.add(field, &format!("Must be at most {max} characters."));
        }
    }
}

fn email(f: &mut Fields, field: &str, v: &mut Option<String>) {
    if let Some(s) = v {
        *s = s.trim().to_ascii_lowercase();
        if !s.is_empty() && !is_valid_email(s) {
            f.add(field, "Enter a valid email address.");
        }
    }
}

fn host(f: &mut Fields, field: &str, v: &mut Option<String>, allow_private: bool) {
    if let Some(s) = v {
        if s.trim().is_empty() {
            s.clear();
        } else {
            match mail_guard::check_host(s, allow_private) {
                Ok(h) => *s = h,
                Err(m) => f.add(field, m),
            }
        }
    }
}

fn port(f: &mut Fields, field: &str, v: Option<i64>) -> Option<i32> {
    v.and_then(|p| match i32::try_from(p) {
        Ok(p) if (1..=65535).contains(&p) => Some(p),
        _ => {
            f.add(field, "Port must be 1 to 65535.");
            None
        }
    })
}

fn password(f: &mut Fields, field: &str, v: &Option<String>) {
    if v.as_deref()
        .is_some_and(|p| p.chars().count() > MAX_PASSWORD_CHARS)
    {
        f.add(
            field,
            &format!("Must be at most {MAX_PASSWORD_CHARS} characters."),
        );
    }
}

/// Validated, normalised settings (ports already range-checked).
pub struct CleanMail {
    f: MailFields,
    smtp_port: Option<i32>,
    imap_port: Option<i32>,
}

impl MailFields {
    pub fn is_empty(&self) -> bool {
        let s = |o: &Option<String>| o.is_none();
        s(&self.reply_to_email)
            && s(&self.smtp_email)
            && s(&self.smtp_username)
            && s(&self.smtp_password)
            && s(&self.smtp_host)
            && self.smtp_port.is_none()
            && s(&self.imap_host)
            && self.imap_port.is_none()
            && s(&self.imap_username)
            && s(&self.imap_password)
            && s(&self.imap_folder)
    }

    pub fn validate(mut self, f: &mut Fields, allow_private: bool) -> CleanMail {
        email(f, "replyToEmail", &mut self.reply_to_email);
        email(f, "smtpEmail", &mut self.smtp_email);
        text(f, "smtpUsername", &mut self.smtp_username, 255);
        text(f, "imapUsername", &mut self.imap_username, 255);
        text(f, "imapFolder", &mut self.imap_folder, 255);
        host(f, "smtpHost", &mut self.smtp_host, allow_private);
        host(f, "imapHost", &mut self.imap_host, allow_private);
        password(f, "smtpPassword", &self.smtp_password);
        password(f, "imapPassword", &self.imap_password);
        let smtp_port = port(f, "smtpPort", self.smtp_port);
        let imap_port = port(f, "imapPort", self.imap_port);
        CleanMail {
            f: self,
            smtp_port,
            imap_port,
        }
    }
}

impl CleanMail {
    pub fn is_empty(&self) -> bool {
        self.f.is_empty()
    }

    /// A changed host, port or username would send the stored password somewhere new, so the
    /// caller must type the password again (or clear it with `""`).
    pub fn require_reentry(&self, cur: &MailSettings, f: &mut Fields) {
        let m = &self.f;
        let changed = |new: Option<&str>, old: Option<&str>| {
            new.is_some_and(|n| !n.is_empty() && Some(n) != old)
        };
        let smtp_changed = changed(m.smtp_host.as_deref(), cur.smtp_host.as_deref())
            || changed(m.smtp_username.as_deref(), cur.smtp_username.as_deref())
            || self.smtp_port.is_some_and(|p| p != cur.smtp_port);
        if smtp_changed && cur.smtp_password_encrypted.is_some() && m.smtp_password.is_none() {
            f.add(
                "smtpPassword",
                "Re-enter the password when changing the host, port or username.",
            );
        }
        let imap_changed = changed(m.imap_host.as_deref(), cur.imap_host.as_deref())
            || changed(m.imap_username.as_deref(), cur.imap_username.as_deref())
            || self.imap_port.is_some_and(|p| Some(p) != cur.imap_port);
        if imap_changed && cur.imap_password_encrypted.is_some() && m.imap_password.is_none() {
            f.add(
                "imapPassword",
                "Re-enter the password when changing the host, port or username.",
            );
        }
    }

    /// Encrypts the passwords for `project` and builds the column update.
    pub fn into_update(self, secrets: &SecretBox, project: sd_db::Id) -> MailUpdate {
        let seal = |kind: &str, p: Option<String>| {
            p.map(|p| {
                if p.is_empty() {
                    p
                } else {
                    secrets.seal(&p, &password_aad(project, kind))
                }
            })
        };
        let m = self.f;
        MailUpdate {
            reply_to_email: m.reply_to_email,
            smtp_email: m.smtp_email,
            smtp_username: m.smtp_username,
            smtp_password_encrypted: seal("smtp", m.smtp_password),
            smtp_host: m.smtp_host,
            smtp_port: self.smtp_port,
            imap_host: m.imap_host,
            imap_port: self.imap_port,
            imap_username: m.imap_username,
            imap_password_encrypted: seal("imap", m.imap_password),
            imap_folder: m.imap_folder,
        }
    }
}

/// Maps a driver error to a fixed message: server banners and OS errors stay in the logs.
fn probe_message(e: &sd_mail::Error) -> &'static str {
    tracing::info!(error = %e, "mail connection test failed");
    match e {
        sd_mail::Error::Timeout(_) => "The server did not answer in time.",
        sd_mail::Error::Tls(_) => "The TLS handshake failed.",
        sd_mail::Error::Io(_) => "Could not connect to the server.",
        _ => "The server refused the connection or the login.",
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ProbeReq {
    host: Option<String>,
    port: Option<i64>,
    /// `starttls` | `implicit` | `none` (`none` only with `SD_ALLOW_PRIVATE_MAIL_HOSTS`).
    tls: Option<String>,
    username: Option<String>,
    password: Option<String>,
}

#[derive(Serialize)]
struct ProbeResult {
    ok: bool,
    message: &'static str,
}

struct Target {
    host: String,
    port: u16,
    tls: TlsMode,
    username: Option<String>,
    password: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Smtp,
    Imap,
}

/// Fills a probe request from the stored settings. The stored password is only used when the
/// target is exactly what is stored; any other target needs its password in the request.
fn resolve(
    s: &AppState,
    req: ProbeReq,
    cur: &MailSettings,
    kind: Kind,
) -> Result<Target, ApiError> {
    let mut f = Fields::default();
    let (cur_host, cur_port, cur_user, cur_pw, name) = match kind {
        Kind::Smtp => (
            cur.smtp_host.clone(),
            Some(cur.smtp_port),
            cur.smtp_username.clone(),
            cur.smtp_password_encrypted.clone(),
            "smtp",
        ),
        Kind::Imap => (
            cur.imap_host.clone(),
            cur.imap_port,
            cur.imap_username.clone(),
            cur.imap_password_encrypted.clone(),
            "imap",
        ),
    };
    let allow = s.config.allow_private_mail_hosts;
    let mut req_host = req.host;
    if req_host.as_deref().is_some_and(|h| h.trim().is_empty()) {
        req_host = None;
    }
    host(&mut f, "host", &mut req_host, allow);
    let host_v = req_host.or(cur_host.clone());
    if host_v.is_none() {
        f.add("host", "A host is required.");
    }
    let port_v = port(&mut f, "port", req.port)
        .or(cur_port)
        .or((kind == Kind::Imap).then_some(993));
    if port_v.is_none() {
        f.add("port", "A port is required.");
    }
    let mut user = req.username;
    text(&mut f, "username", &mut user, 255);
    let user_v = user.filter(|u| !u.is_empty()).or(cur_user.clone());
    password(&mut f, "password", &req.password);
    let tls = match req.tls.as_deref() {
        None => port_v.map(|p| tls_for_port(p as u16)),
        Some("starttls") => Some(TlsMode::StartTls),
        Some("implicit") => Some(TlsMode::Implicit),
        Some("none") if allow => Some(TlsMode::None),
        Some(_) => {
            f.add("tls", "Must be starttls or implicit.");
            None
        }
    };
    let same_target = host_v == cur_host && port_v == cur_port && user_v == cur_user;
    let password_v = match req.password.filter(|p| !p.is_empty()) {
        Some(p) => Some(p),
        None if same_target => cur_pw.and_then(|c| {
            s.secrets
                .open(&c, &password_aad(cur.project_id, name))
                .inspect_err(|e| tracing::error!(error = %e, "stored mail password unreadable"))
                .ok()
        }),
        None => {
            f.add(
                "password",
                "Enter the password to test a different server or user.",
            );
            None
        }
    };
    f.finish()?;
    Ok(Target {
        host: host_v.unwrap_or_default(),
        port: port_v.unwrap_or_default() as u16,
        tls: tls.unwrap_or(TlsMode::StartTls),
        username: user_v,
        password: password_v,
    })
}

async fn load(
    s: &AppState,
    ip: std::net::IpAddr,
    user: sd_db::Id,
    id: sd_db::Id,
) -> Result<MailSettings, ApiError> {
    s.limiter.check(Rule::MailProbe, ip)?;
    project_admin(s, user, id).await?;
    projects::get_mail_settings(&s.pool, user, id)
        .await?
        .ok_or_else(|| ApiError::not_found("Project"))
}

async fn guard(s: &AppState, t: &Target) -> Result<(), ApiError> {
    mail_guard::check_resolved(&t.host, t.port, s.config.allow_private_mail_hosts)
        .await
        .map_err(|m| ApiError::Validation([("host".to_string(), m.to_string())].into()))
}

async fn test_smtp(
    State(s): State<AppState>,
    ClientIp(ip): ClientIp,
    AuthUser(user): AuthUser,
    ApiPath(id): ApiPath<sd_db::Id>,
    ApiJson(req): ApiJson<ProbeReq>,
) -> Result<Envelope<ProbeResult>, ApiError> {
    let cur = load(&s, ip, user.id, id).await?;
    let t = resolve(&s, req, &cur, Kind::Smtp)?;
    guard(&s, &t).await?;
    let from = cur
        .smtp_email
        .clone()
        .unwrap_or_else(|| "probe@example.invalid".into());
    let mut cfg = SmtpConfig::new(&t.host, t.port, t.tls, from);
    cfg.allow_private = s.config.allow_private_mail_hosts;
    cfg.username = t.username;
    cfg.password = t.password;
    cfg.io_timeout = PROBE_TIMEOUT;
    cfg.total_timeout = PROBE_TIMEOUT * 2;
    let r = sd_mail::check_smtp(&cfg).await;
    Ok(Envelope::ok(result(r)))
}

async fn test_imap(
    State(s): State<AppState>,
    ClientIp(ip): ClientIp,
    AuthUser(user): AuthUser,
    ApiPath(id): ApiPath<sd_db::Id>,
    ApiJson(req): ApiJson<ProbeReq>,
) -> Result<Envelope<ProbeResult>, ApiError> {
    let cur = load(&s, ip, user.id, id).await?;
    let t = resolve(&s, req, &cur, Kind::Imap)?;
    guard(&s, &t).await?;
    let mut cfg = ImapConfig::new(
        &t.host,
        t.port,
        t.tls,
        t.username.unwrap_or_default(),
        t.password.unwrap_or_default(),
    );
    cfg.folder = cur.imap_folder.clone();
    cfg.allow_private = s.config.allow_private_mail_hosts;
    cfg.connect_timeout = PROBE_TIMEOUT;
    cfg.io_timeout = PROBE_TIMEOUT;
    let r = match ImapMailbox::connect(&cfg).await {
        Ok(mb) => {
            let _ = mb.logout().await;
            Ok(())
        }
        Err(e) => Err(e),
    };
    Ok(Envelope::ok(result(r)))
}

fn result(r: Result<(), sd_mail::Error>) -> ProbeResult {
    match r {
        Ok(()) => ProbeResult {
            ok: true,
            message: "Connection succeeded.",
        },
        Err(e) => ProbeResult {
            ok: false,
            message: probe_message(&e),
        },
    }
}
