use thiserror::Error;

/// Why an inbound message was rejected. Typed so callers can log or bounce precisely.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("message is {size} bytes, limit is {limit}")]
    MessageTooLarge { size: usize, limit: usize },
    #[error("message has {count} header fields, limit is {limit}")]
    TooManyHeaders { count: usize, limit: usize },
    #[error("message has more than {limit} attachments")]
    TooManyAttachments { limit: usize },
    #[error("attachment {filename:?} is {size} bytes, limit is {limit}")]
    AttachmentTooLarge {
        filename: String,
        size: usize,
        limit: usize,
    },
    #[error("not a parseable RFC 5322 message")]
    Unparseable,
}

#[derive(Debug, Error)]
pub enum Error {
    /// A header value, address or name contained CR/LF or another control character.
    #[error("invalid {field}: control characters are not allowed")]
    HeaderInjection { field: &'static str },
    #[error("invalid {field}: {reason}")]
    Invalid { field: &'static str, reason: String },
    #[error("smtp: {0}")]
    Smtp(String),
    #[error("imap: {0}")]
    Imap(String),
    #[error("tls: {0}")]
    Tls(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("timed out: {0}")]
    Timeout(&'static str),
    #[error(transparent)]
    Parse(#[from] ParseError),
}
