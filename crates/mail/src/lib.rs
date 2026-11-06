//! sd-mail: SMTP sender (lettre), MIME parsing (mail-parser) and IMAP poller (async-imap).
//!
//! No IO on global state: every call takes its own connection settings, so one process serves
//! many projects with different mail servers.

mod error;
pub mod guard;
mod imap;
mod inbound;
mod outbound;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use error::{Error, ParseError};
pub use imap::{
    ImapConfig, ImapMailbox, Mailbox, PollOptions, PollOutcome, PollState, Polled, RawMessage, poll,
};
pub use inbound::{InboundAttachment, InboundEmail, Limits, parse_inbound};
pub use outbound::{
    OutAttachment, OutboundEmail, SmtpConfig, build_message, check as check_smtp, send,
};

use std::time::Duration;

/// Transport security for SMTP and IMAP connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsMode {
    /// Plain connection upgraded with STARTTLS (SMTP 587, IMAP 143).
    StartTls,
    /// TLS from the first byte (SMTP 465, IMAP 993).
    Implicit,
    /// No encryption. Local test servers only.
    None,
}

pub(crate) const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const DEFAULT_IO_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(test)]
mod tests;
