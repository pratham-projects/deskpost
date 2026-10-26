use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::str::FromStr;
use std::time::Duration;

use lettre::message::header::{ContentType, HeaderName, HeaderValue};
use lettre::message::{Attachment, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::Tls;
use lettre::{Address, AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::{DEFAULT_IO_TIMEOUT, Error, TlsMode};

/// Per-call SMTP settings (one per project; nothing is global).
#[derive(Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub tls: TlsMode,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from_name: Option<String>,
    pub from_address: String,
    /// Per SMTP command (connect, EHLO, DATA, ...).
    pub io_timeout: Duration,
    /// Whole `send` call, so a slow server cannot hold a worker forever.
    pub total_timeout: Duration,
}

impl std::fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("tls", &self.tls)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("from_address", &self.from_address)
            .finish_non_exhaustive()
    }
}

impl SmtpConfig {
    pub fn new(
        host: impl Into<String>,
        port: u16,
        tls: TlsMode,
        from_address: impl Into<String>,
    ) -> Self {
        Self {
            host: host.into(),
            port,
            tls,
            username: None,
            password: None,
            from_name: None,
            from_address: from_address.into(),
            io_timeout: DEFAULT_IO_TIMEOUT,
            total_timeout: 2 * DEFAULT_IO_TIMEOUT,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OutAttachment {
    pub filename: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct OutboundEmail {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub text: Option<String>,
    pub html: Option<String>,
    /// Generated (`<random@from-domain>`) when `None`.
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    /// Extra headers; names that the builder owns (From, Subject, ...) are rejected.
    pub headers: Vec<(String, String)>,
    pub attachments: Vec<OutAttachment>,
}

/// Headers the builder sets itself; a custom header with one of these names is an error.
const RESERVED: &[&str] = &[
    "from",
    "to",
    "cc",
    "bcc",
    "subject",
    "date",
    "message-id",
    "in-reply-to",
    "references",
    "reply-to",
    "sender",
    "mime-version",
    "content-type",
    "content-transfer-encoding",
    "content-disposition",
];

fn no_control(field: &'static str, v: &str) -> Result<(), Error> {
    if v.chars().any(char::is_control) {
        Err(Error::HeaderInjection { field })
    } else {
        Ok(())
    }
}

fn mailbox(field: &'static str, name: Option<&str>, addr: &str) -> Result<Mailbox, Error> {
    no_control(field, addr)?;
    if let Some(n) = name {
        no_control(field, n)?;
    }
    let address = Address::from_str(addr.trim()).map_err(|e| Error::Invalid {
        field,
        reason: e.to_string(),
    })?;
    Ok(Mailbox::new(
        name.filter(|n| !n.is_empty()).map(str::to_owned),
        address,
    ))
}

fn angle(field: &'static str, id: &str) -> Result<String, Error> {
    no_control(field, id)?;
    let id = id.trim();
    if id.is_empty() || id.contains(char::is_whitespace) {
        return Err(Error::Invalid {
            field,
            reason: "empty or contains whitespace".into(),
        });
    }
    Ok(if id.starts_with('<') {
        id.to_owned()
    } else {
        format!("<{id}>")
    })
}

fn random_id() -> String {
    let (a, b) = (
        RandomState::new().build_hasher().finish(),
        RandomState::new().build_hasher().finish(),
    );
    format!("{a:016x}{b:016x}")
}

/// Build the MIME message. Returns it with its Message-ID.
pub fn build_message(cfg: &SmtpConfig, mail: &OutboundEmail) -> Result<(Message, String), Error> {
    let from = mailbox("from", cfg.from_name.as_deref(), &cfg.from_address)?;
    let mut b = Message::builder().from(from.clone());
    if mail.to.is_empty() && mail.cc.is_empty() && mail.bcc.is_empty() {
        return Err(Error::Invalid {
            field: "to",
            reason: "no recipients".into(),
        });
    }
    for a in &mail.to {
        b = b.to(mailbox("to", None, a)?);
    }
    for a in &mail.cc {
        b = b.cc(mailbox("cc", None, a)?);
    }
    for a in &mail.bcc {
        b = b.bcc(mailbox("bcc", None, a)?);
    }
    no_control("subject", &mail.subject)?;
    b = b.subject(mail.subject.clone());

    let message_id = match &mail.message_id {
        Some(id) => angle("message_id", id)?,
        None => format!("<{}@{}>", random_id(), from.email.domain()),
    };
    b = b.message_id(Some(message_id.clone()));
    if let Some(r) = &mail.in_reply_to {
        b = b.in_reply_to(angle("in_reply_to", r)?);
    }
    if !mail.references.is_empty() {
        let refs = mail
            .references
            .iter()
            .map(|r| angle("references", r))
            .collect::<Result<Vec<_>, _>>()?;
        b = b.references(refs.join(" "));
    }

    let body = match (&mail.text, &mail.html) {
        (Some(t), Some(h)) => MultiPart::alternative_plain_html(t.clone(), h.clone()),
        (Some(t), None) => MultiPart::mixed().singlepart(SinglePart::plain(t.clone())),
        (None, Some(h)) => MultiPart::mixed().singlepart(SinglePart::html(h.clone())),
        (None, None) => {
            return Err(Error::Invalid {
                field: "body",
                reason: "need text or html".into(),
            });
        }
    };
    let body = if mail.attachments.is_empty() {
        body
    } else {
        let mut mixed = MultiPart::mixed().multipart(body);
        for a in &mail.attachments {
            no_control("attachment filename", &a.filename)?;
            no_control("attachment content type", &a.content_type)?;
            let ct = ContentType::parse(&a.content_type).map_err(|e| Error::Invalid {
                field: "attachment content type",
                reason: e.to_string(),
            })?;
            mixed = mixed.singlepart(Attachment::new(a.filename.clone()).body(a.bytes.clone(), ct));
        }
        mixed
    };
    let mut msg = b.multipart(body).map_err(|e| Error::Invalid {
        field: "message",
        reason: e.to_string(),
    })?;

    for (name, value) in &mail.headers {
        let valid_name = !name.is_empty()
            && name.len() <= 76
            && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-');
        if !valid_name || RESERVED.contains(&name.to_ascii_lowercase().as_str()) {
            return Err(Error::Invalid {
                field: "header name",
                reason: name.clone(),
            });
        }
        // Non-ASCII would need RFC 2047 encoding; raw bytes in a header are invalid.
        if !value.is_ascii() {
            return Err(Error::Invalid {
                field: "header value",
                reason: format!("{name}: must be ASCII"),
            });
        }
        no_control("header value", value)?;
        let hn = HeaderName::new_from_ascii(name.clone()).map_err(|e| Error::Invalid {
            field: "header name",
            reason: e.to_string(),
        })?;
        msg.headers_mut()
            .insert_raw(HeaderValue::new(hn, value.clone()));
    }
    Ok((msg, message_id))
}

fn transport(cfg: &SmtpConfig) -> Result<AsyncSmtpTransport<Tokio1Executor>, Error> {
    let smtp = |e: lettre::transport::smtp::Error| Error::Smtp(e.to_string());
    let builder = match cfg.tls {
        TlsMode::None => {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.host).tls(Tls::None)
        }
        TlsMode::Implicit => {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.host).map_err(smtp)?
        }
        TlsMode::StartTls => {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.host).map_err(smtp)?
        }
    };
    let mut builder = builder.port(cfg.port).timeout(Some(cfg.io_timeout));
    if let (Some(u), Some(p)) = (&cfg.username, &cfg.password) {
        builder = builder.credentials(Credentials::new(u.clone(), p.clone()));
    }
    Ok(builder.build())
}

/// Send one message; returns its Message-ID.
pub async fn send(cfg: &SmtpConfig, mail: &OutboundEmail) -> Result<String, Error> {
    let (msg, id) = build_message(cfg, mail)?;
    let t = transport(cfg)?;
    tokio::time::timeout(cfg.total_timeout, t.send(msg))
        .await
        .map_err(|_| Error::Timeout("smtp send"))?
        .map_err(|e| Error::Smtp(e.to_string()))?;
    Ok(id)
}
