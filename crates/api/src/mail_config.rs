//! Mail and attachment settings (R23), kept apart from the core `Config` fields.

use std::time::Duration;

use crate::config::{num, secret};

const MB: usize = 1024 * 1024;

#[derive(Clone)]
pub struct MailConfig {
    /// `SD_WEBHOOK_SECRET`; without it `POST /incoming-email` answers 404.
    pub webhook_secret: Option<Vec<u8>>,
    /// `SD_MAIL_PLAINTEXT=1`: never use TLS to project mail servers (local dev and tests only).
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
            get("SD_WEBHOOK_SECRET").map(|_| secret(get, "SD_WEBHOOK_SECRET", errs));
        let plaintext = match get("SD_MAIL_PLAINTEXT").as_deref() {
            None | Some("0" | "false") => false,
            Some("1" | "true") => true,
            Some(_) => {
                errs.push("SD_MAIL_PLAINTEXT must be true or false".into());
                false
            }
        };
        Self {
            webhook_secret,
            plaintext,
            max_file_bytes: num(
                get,
                "SD_MAX_FILE_BYTES",
                d.max_file_bytes,
                1,
                512 * MB,
                errs,
            ),
            max_request_bytes: num(
                get,
                "SD_MAX_REQUEST_BYTES",
                d.max_request_bytes,
                1,
                512 * MB,
                errs,
            ),
            max_files: num(get, "SD_MAX_FILES", d.max_files, 1, 100, errs),
            imap_poll_interval: Duration::from_secs(num(
                get,
                "SD_IMAP_POLL_SECS",
                60,
                5,
                86_400,
                errs,
            ) as u64),
            imap_concurrency: num(get, "SD_IMAP_CONCURRENCY", d.imap_concurrency, 1, 32, errs),
            download_ttl: d.download_ttl,
        }
    }
}
