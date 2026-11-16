//! Mail and attachment settings (R23), kept apart from the core `Config` fields.

use std::time::Duration;

use crate::config::{num, secret};

const MB: usize = 1024 * 1024;

/// Platform mailbox for account mail (password reset), never for tenant mail. Operator-supplied,
/// so private addresses are allowed (an internal relay is the usual setup).
#[derive(Clone)]
pub struct SystemSmtp {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from: String,
}

impl std::fmt::Debug for SystemSmtp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemSmtp")
            .field("host", &self.host)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct MailConfig {
    /// `DP_SYSTEM_SMTP_*`; without a host, password reset is switched off.
    pub system_smtp: Option<SystemSmtp>,
    /// `DP_WEBHOOK_SECRET`; without it `POST /incoming-email` answers 404.
    pub webhook_secret: Option<Vec<u8>>,
    /// `DP_MAIL_PLAINTEXT=1`: never use TLS to project mail servers (local dev and tests only).
    pub plaintext: bool,
    pub max_file_bytes: usize,
    /// Per upload request and per inbound email.
    pub max_request_bytes: usize,
    pub max_files: usize,
    pub imap_poll_interval: Duration,
    pub imap_concurrency: usize,
    /// Signed download links live this long.
    pub download_ttl: Duration,
}

impl Default for MailConfig {
    fn default() -> Self {
        Self {
            system_smtp: None,
            webhook_secret: None,
            plaintext: false,
            max_file_bytes: 10 * MB,
            max_request_bytes: 25 * MB,
            max_files: 20,
            imap_poll_interval: Duration::from_secs(60),
            imap_concurrency: 4,
            download_ttl: Duration::from_secs(300),
        }
    }
}

impl MailConfig {
    pub fn from_lookup(get: &impl Fn(&str) -> Option<String>, errs: &mut Vec<String>) -> Self {
        let d = Self::default();
        let webhook_secret =
            get("DP_WEBHOOK_SECRET").map(|_| secret(get, "DP_WEBHOOK_SECRET", errs));
        let plaintext = match get("DP_MAIL_PLAINTEXT").as_deref() {
            None | Some("0" | "false") => false,
            Some("1" | "true") => true,
            Some(_) => {
                errs.push("DP_MAIL_PLAINTEXT must be true or false".into());
                false
            }
        };
        Self {
            system_smtp: system_smtp(get, errs),
            webhook_secret,
            plaintext,
            max_file_bytes: num(
                get,
                "DP_MAX_FILE_BYTES",
                d.max_file_bytes,
                1,
                512 * MB,
                errs,
            ),
            max_request_bytes: num(
                get,
                "DP_MAX_REQUEST_BYTES",
                d.max_request_bytes,
                1,
                512 * MB,
                errs,
            ),
            max_files: num(get, "DP_MAX_FILES", d.max_files, 1, 100, errs),
            imap_poll_interval: Duration::from_secs(num(
                get,
                "DP_IMAP_POLL_SECS",
                60,
                5,
                86_400,
                errs,
            ) as u64),
            imap_concurrency: num(get, "DP_IMAP_CONCURRENCY", d.imap_concurrency, 1, 32, errs),
            download_ttl: d.download_ttl,
        }
    }
}

fn system_smtp(
    get: &impl Fn(&str) -> Option<String>,
    errs: &mut Vec<String>,
) -> Option<SystemSmtp> {
    let host = get("DP_SYSTEM_SMTP_HOST");
    let other = [
        "DP_SYSTEM_SMTP_PORT",
        "DP_SYSTEM_SMTP_USER",
        "DP_SYSTEM_SMTP_PASSWORD",
        "DP_SYSTEM_SMTP_FROM",
    ]
    .into_iter()
    .any(|k| get(k).is_some());
    let Some(host) = host else {
        if other {
            errs.push("DP_SYSTEM_SMTP_HOST is required when other DP_SYSTEM_SMTP_* are set".into());
        }
        return None;
    };
    let from = get("DP_SYSTEM_SMTP_FROM").unwrap_or_else(|| {
        errs.push("DP_SYSTEM_SMTP_FROM is required with DP_SYSTEM_SMTP_HOST".into());
        String::new()
    });
    if !from.is_empty() && !dp_core::email_addr::is_valid_email(&from) {
        errs.push("DP_SYSTEM_SMTP_FROM must be an email address".into());
    }
    let username = get("DP_SYSTEM_SMTP_USER");
    let password = get("DP_SYSTEM_SMTP_PASSWORD");
    if username.is_some() != password.is_some() {
        errs.push("DP_SYSTEM_SMTP_USER and DP_SYSTEM_SMTP_PASSWORD go together".into());
    }
    Some(SystemSmtp {
        host,
        port: num(get, "DP_SYSTEM_SMTP_PORT", 587, 1, u16::MAX as usize, errs) as u16,
        username,
        password,
        from,
    })
}
