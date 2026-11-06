//! Fixtures for the R23 mail and attachment tests: an app with a temp storage dir, plain-text mail
//! servers, projects with sealed SMTP/IMAP credentials, tickets and helpers for MIME text.
#![allow(dead_code)]

use std::sync::Arc;

use api::mail_out::{Mailer, RecordingMailer, imap_aad, smtp_aad};
use api::{AppState, build_app};
use sd_db::projects::Project;
use sd_db::tickets::{Access, NewTicket, Ticket};
use sd_db::{Id, Pool, Priority, Role, TicketSource, tickets};
use tempfile::TempDir;

use crate::common::{Session, TestApp, test_config};

pub const WEBHOOK_SECRET: &str = "wH7kP2mQx9Lr4Zb8Nc3Tv6Yd1Gs5Fj0A";
pub const MAILBOX: &str = "support@acme.test";

pub struct Kit {
    pub t: TestApp,
    pub dir: TempDir,
    pub recorder: Arc<RecordingMailer>,
}

impl Kit {
    /// Real SMTP mailer (talks to whatever the project is configured with).
    pub async fn new(pool: Pool) -> Self {
        Self::build(pool, false, |_| {}).await
    }

    /// Mailer swapped for the recording double.
    pub async fn recording(pool: Pool) -> Self {
        Self::build(pool, true, |_| {}).await
    }

    pub async fn build(
        pool: Pool,
        recording: bool,
        tweak: impl FnOnce(&mut api::config::Config),
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut config = test_config();
        config.storage_dir = dir.path().join("files");
        config.mail.webhook_secret = Some(WEBHOOK_SECRET.as_bytes().to_vec());
        config.mail.plaintext = true;
        config.allow_private_mail_hosts = true;
        tweak(&mut config);
        let mut state = AppState::new(pool.clone(), config).unwrap();
        let recorder = Arc::new(RecordingMailer::default());
        if recording {
            state.mailer = Mailer::Recording(recorder.clone());
        }
        let app = build_app(state.clone());
        Self {
            t: TestApp { app, state, pool },
            dir,
            recorder,
        }
    }

    /// Owner (project admin), project and a ticket for `client@example.com`.
    pub async fn tenant(
        &self,
        owner: &str,
        slug: &str,
    ) -> (sd_db::users::User, Project, Ticket, Session) {
        let user = self.t.user(owner, Role::Agent).await;
        let project = self.t.project(user.id, slug).await;
        let ticket = self.ticket(&user, &project, "client@example.com").await;
        let session = self.t.signin(owner).await;
        (user, project, ticket, session)
    }

    pub async fn ticket(
        &self,
        actor: &sd_db::users::User,
        project: &Project,
        client: &str,
    ) -> Ticket {
        tickets::create(
            &self.t.pool,
            &Access::Member(actor.id),
            &NewTicket {
                project_id: project.id,
                client_email: client.into(),
                client_name: None,
                subject: "Printer on fire".into(),
                description: Some("It is burning".into()),
                priority: Priority::Normal,
                source: TicketSource::Web,
                created_by: None,
                first_message: "It is burning".into(),
            },
        )
        .await
        .unwrap()
        .unwrap()
    }

    /// Outbound mail through `smtp_port` (with credentials u/pw), optional inbound over IMAP.
    pub async fn configure_mail(
        &self,
        project: Id,
        smtp_port: u16,
        imap: Option<(u16, &str, &str)>,
    ) {
        let s = &self.t.state;
        let smtp_pw = s.secrets.seal("smtp-secret-pw", &smtp_aad(project));
        sqlx::query(
            "UPDATE projects SET smtp_email = 'support@' || slug || '.test', smtp_host = '127.0.0.1',
                    smtp_port = $2, smtp_username = 'u', smtp_password_encrypted = $3 WHERE id = $1",
        )
        .bind(project)
        .bind(i32::from(smtp_port))
        .bind(smtp_pw)
        .execute(&self.t.pool)
        .await
        .unwrap();
        if let Some((port, user, pass)) = imap {
            sqlx::query(
                "UPDATE projects SET imap_host = '127.0.0.1', imap_port = $2, imap_username = $3,
                        imap_password_encrypted = $4 WHERE id = $1",
            )
            .bind(project)
            .bind(i32::from(port))
            .bind(user)
            .bind(s.secrets.seal(pass, &imap_aad(project)))
            .execute(&self.t.pool)
            .await
            .unwrap();
        }
    }

    pub async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&self.t.pool)
            .await
            .unwrap()
    }

    /// Files under the storage root (not counting directories or the temp dir).
    pub fn stored_files(&self) -> usize {
        fn walk(p: &std::path::Path) -> usize {
            std::fs::read_dir(p).map_or(0, |d| {
                d.flatten()
                    .map(|e| {
                        if e.path().is_dir() {
                            walk(&e.path())
                        } else {
                            1
                        }
                    })
                    .sum()
            })
        }
        walk(&self.dir.path().join("files"))
    }
}

/// A port nothing listens on.
pub fn closed_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

/// Plain-text RFC 5322 message.
pub fn mime(headers: &[(&str, &str)], body: &str) -> Vec<u8> {
    let mut out = String::new();
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    out.push_str(body);
    out.into_bytes()
}

/// A customer mail to the project mailbox.
pub fn customer_mail(
    message_id: &str,
    subject: &str,
    body: &str,
    extra: &[(&str, &str)],
) -> Vec<u8> {
    let mut h = vec![
        ("From", "Ann Client <ann@example.com>"),
        ("To", MAILBOX),
        ("Subject", subject),
        ("Message-ID", message_id),
        ("Content-Type", "text/plain; charset=utf-8"),
    ];
    h.extend_from_slice(extra);
    mime(&h, body)
}
