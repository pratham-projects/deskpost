//! Outbound mail seam. Routes call [`OutboundMailer`] after the database transaction has
//! committed; R23 implements it with `sd-mail`. A failure never fails the request.

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use sd_db::Id;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A public agent reply that was saved and should reach the ticket's client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyMail {
    pub project_id: Id,
    pub ticket_id: Id,
    pub ticket_number: i64,
    pub message_id: Id,
    pub sent_by: Id,
    pub to: String,
    pub ticket_subject: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailError(pub String);

impl std::fmt::Display for MailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub trait OutboundMailer: Send + Sync {
    fn send_reply<'a>(&'a self, mail: &'a ReplyMail) -> BoxFuture<'a, Result<(), MailError>>;
}

/// Default until R23 wires `sd-mail`: accepts and drops.
pub struct NoopMailer;

impl OutboundMailer for NoopMailer {
    fn send_reply<'a>(&'a self, _: &'a ReplyMail) -> BoxFuture<'a, Result<(), MailError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Test double: remembers every mail and optionally fails them all.
#[derive(Default)]
pub struct RecordingMailer {
    sent: Mutex<Vec<ReplyMail>>,
    fail_with: Option<String>,
}

impl RecordingMailer {
    pub fn failing(error: &str) -> Self {
        Self {
            fail_with: Some(error.into()),
            ..Self::default()
        }
    }

    pub fn sent(&self) -> Vec<ReplyMail> {
        self.sent.lock().unwrap().clone()
    }
}

impl OutboundMailer for RecordingMailer {
    fn send_reply<'a>(&'a self, mail: &'a ReplyMail) -> BoxFuture<'a, Result<(), MailError>> {
        self.sent.lock().unwrap().push(mail.clone());
        let result = match &self.fail_with {
            Some(e) => Err(MailError(e.clone())),
            None => Ok(()),
        };
        Box::pin(async move { result })
    }
}
