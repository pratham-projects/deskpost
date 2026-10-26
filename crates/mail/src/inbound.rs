use mail_parser::MessageParser;
use sha2::{Digest, Sha256};

use crate::ParseError;

/// Caps applied before and while parsing untrusted mail.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_message_bytes: usize,
    pub max_attachment_bytes: usize,
    pub max_attachments: usize,
    pub max_headers: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_message_bytes: 25 * 1024 * 1024,
            max_attachment_bytes: 10 * 1024 * 1024,
            max_attachments: 20,
            max_headers: 200,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundAttachment {
    pub filename: String,
    pub content_type: String,
    /// Content-ID without angle brackets, set for inline images (`cid:` references).
    pub content_id: Option<String>,
    pub inline: bool,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundEmail {
    /// Without angle brackets. Derived from a hash of the raw bytes when the header is missing,
    /// so re-fetching the same message yields the same id.
    pub message_id: String,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub from: Option<String>,
    pub from_name: Option<String>,
    pub to: Vec<String>,
    pub subject: String,
    pub text: Option<String>,
    pub html: Option<String>,
    pub attachments: Vec<InboundAttachment>,
    /// Unix seconds (UTC).
    pub date: Option<i64>,
}

fn strip_brackets(s: &str) -> String {
    s.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim()
        .to_owned()
}

pub fn parse_inbound(raw: &[u8], limits: &Limits) -> Result<InboundEmail, ParseError> {
    if raw.len() > limits.max_message_bytes {
        return Err(ParseError::MessageTooLarge {
            size: raw.len(),
            limit: limits.max_message_bytes,
        });
    }
    let msg = MessageParser::new()
        .parse(raw)
        .ok_or(ParseError::Unparseable)?;
    let count = msg.header_count();
    if count > limits.max_headers {
        return Err(ParseError::TooManyHeaders {
            count,
            limit: limits.max_headers,
        });
    }
    // A bare blob with no header at all still "parses"; require something mail-shaped.
    if count == 0 {
        return Err(ParseError::Unparseable);
    }

    let mut attachments = Vec::new();
    for part in msg.attachments() {
        if attachments.len() >= limits.max_attachments {
            return Err(ParseError::TooManyAttachments {
                limit: limits.max_attachments,
            });
        }
        let filename = part.attachment_name().unwrap_or("attachment").to_owned();
        let size = part.decoded_len();
        if size > limits.max_attachment_bytes {
            return Err(ParseError::AttachmentTooLarge {
                filename,
                size,
                limit: limits.max_attachment_bytes,
            });
        }
        let content_type = part
            .content_type()
            .map(|c| match c.subtype() {
                Some(s) => format!("{}/{}", c.ctype(), s).to_ascii_lowercase(),
                None => c.ctype().to_ascii_lowercase(),
            })
            .unwrap_or_else(|| "application/octet-stream".into());
        attachments.push(InboundAttachment {
            filename,
            content_type,
            content_id: part.content_id().map(strip_brackets),
            inline: part.is_inline(),
            bytes: part.decoded().into_owned(),
        });
    }

    let message_id = match msg.message_id().map(strip_brackets) {
        Some(id) if !id.is_empty() => id,
        _ => {
            let digest = Sha256::digest(raw);
            let hex: String = digest.iter().take(16).map(|b| format!("{b:02x}")).collect();
            format!("{hex}@generated.invalid")
        }
    };
    let from = msg.from().and_then(|a| a.first());
    Ok(InboundEmail {
        message_id,
        in_reply_to: msg
            .in_reply_to()
            .and_then(|l| l.iter().next().map(strip_brackets))
            .filter(|s| !s.is_empty()),
        references: msg
            .references()
            .map(|l| {
                l.iter()
                    .map(strip_brackets)
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        from: from.and_then(|m| m.address()).map(str::to_owned),
        from_name: from.and_then(|m| m.name()).map(str::to_owned),
        to: msg
            .to()
            .map(|l| {
                l.mailboxes()
                    .filter_map(|m| m.address().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        subject: msg.subject().unwrap_or_default().to_owned(),
        text: msg.body_text(0).map(|c| c.into_owned()),
        // body_html(0) converts a text-only message to HTML; only report a real text/html part.
        html: msg
            .html_body()
            .find(|p| p.is_content_type("text", "html"))
            .and_then(|p| p.text())
            .map(|c| c.into_owned()),
        attachments,
        date: msg.date().map(|d| d.to_timestamp()),
    })
}
