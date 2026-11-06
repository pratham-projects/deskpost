//! Background jobs started by `serve`: IMAP polling, the auto-response outbox and auto-close.
//! Every job does one bounded round per call (`*_once`), so tests drive them directly.

use std::sync::Arc;
use std::time::Duration;

use sd_db::projects::MailSettings;
use sd_db::{mailq, projects, tickets};
use sd_mail::{ImapConfig, ImapMailbox, PollOptions, PollState, poll};
use tokio::sync::Semaphore;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{MissedTickBehavior, interval};

use crate::ingest::{Outcome, inbound_limits, ingest};
use crate::mail_out::{imap_aad, redact, run_outbox_once, tls_for};
use crate::state::AppState;

/// IMAP fetch size per round.
pub const IMAP_BATCH: usize = 50;
/// Rounds per project and tick; a bigger backlog continues on the next tick.
const MAX_ROUNDS: usize = 20;
/// One project's whole poll; a hung server must not hold a permit (and the tick) for ever.
const POLL_DEADLINE: Duration = Duration::from_secs(120);
const OUTBOX_EVERY: Duration = Duration::from_secs(15);
const HOUSEKEEPING_EVERY: Duration = Duration::from_secs(3600);
const OUTBOX_RETENTION_DAYS: i32 = 30;

/// The database refused the data itself (bad value, NUL byte, too long, check/not-null violation),
/// as opposed to a pool, IO or serialization failure that a retry can fix.
pub(crate) fn is_data_error(e: &sd_db::Error) -> bool {
    matches!(e, sd_db::Error::Database(d)
        if d.code().is_some_and(|c| c.starts_with("22") || c == "23514" || c == "23502"))
}

/// Poll one project's mailbox until it is drained (or `MAX_ROUNDS`). The cursor is stored after each
/// round and only when every mail of the round was handled, so a database error replays the round
/// (the Message-ID key makes that harmless). Returns how many mails became tickets or replies.
pub async fn poll_project(state: &AppState, s: &MailSettings) -> Result<usize, String> {
    let (Some(host), Some(user), Some(sealed)) = (
        s.imap_host.as_deref(),
        s.imap_username.as_deref(),
        s.imap_password_encrypted.as_deref(),
    ) else {
        return Err("IMAP not configured".into());
    };
    let password = state
        .secrets
        .open(sealed, &imap_aad(s.project_id))
        .map_err(|_| "cannot open the stored IMAP password".to_string())?;
    let clean = |e: &dyn std::fmt::Display| redact(&e.to_string(), &[&password]);
    let port =
        u16::try_from(s.imap_port.unwrap_or(993)).map_err(|_| "bad IMAP port".to_string())?;
    let mut cfg = ImapConfig::new(
        host,
        port,
        tls_for(port, state.config.mail.plaintext, 993),
        user,
        &password,
    );
    cfg.folder = s.imap_folder.clone();
    cfg.allow_private = state.config.allow_private_mail_hosts;
    cfg.max_message_bytes = state.config.mail.max_request_bytes;
    let mut mailbox = ImapMailbox::connect(&cfg).await.map_err(|e| clean(&e))?;

    let opts = PollOptions {
        batch_size: IMAP_BATCH,
        mark_seen: false,
        limits: inbound_limits(&state.config),
    };
    let mut cursor = PollState {
        uid_validity: s.imap_uid_validity.and_then(|v| u32::try_from(v).ok()),
        last_uid: u32::try_from(s.imap_last_uid).unwrap_or(0),
    };
    let mut handled = 0;
    for _ in 0..MAX_ROUNDS {
        let out = poll(&mut mailbox, cursor, &opts)
            .await
            .map_err(|e| clean(&e))?;
        for polled in &out.emails {
            match &polled.email {
                Ok(email) => match ingest(state, s.project_id, email, None).await {
                    Ok(Outcome::Created { .. } | Outcome::Replied { .. }) => handled += 1,
                    Ok(_) => {}
                    // A mail whose own content the database rejects can never succeed: consume it,
                    // or it would wedge the mailbox. Connection-level errors still abort the round.
                    Err(e) if is_data_error(&e) => {
                        tracing::warn!(project = %s.project_id, uid = polled.uid, error = %e, "inbound mail not storable; skipped")
                    }
                    Err(e) => return Err(clean(&e)),
                },
                // Consumed on purpose: retrying a hostile or oversize message forever helps nobody.
                Err(e) => {
                    tracing::warn!(project = %s.project_id, uid = polled.uid, error = %e, "inbound mail rejected")
                }
            }
        }
        cursor = out.state;
        projects::set_imap_cursor(
            &state.pool,
            s.project_id,
            i64::from(cursor.uid_validity.unwrap_or(0)),
            i64::from(cursor.last_uid),
        )
        .await
        .map_err(|e| clean(&e))?;
        if !out.has_more {
            break;
        }
    }
    Ok(handled)
}

/// One tick: every IMAP-enabled project, at most `imap_concurrency` at a time. Each project's
/// outcome (error text without secrets, or cleared) is recorded on the project. Returns projects polled.
pub async fn poll_all(state: &AppState) -> sd_db::Result<usize> {
    let all = projects::list_imap_enabled(&state.pool).await?;
    let permits = Arc::new(Semaphore::new(state.config.mail.imap_concurrency));
    let mut set = JoinSet::new();
    for s in all {
        let (state, permits) = (state.clone(), permits.clone());
        set.spawn(async move {
            let _permit = permits.acquire_owned().await;
            let result = tokio::time::timeout(POLL_DEADLINE, poll_project(&state, &s))
                .await
                .unwrap_or_else(|_| Err("polling timed out".to_string()));
            let note = result.as_ref().err().map(String::as_str);
            if let Err(e) = mailq::set_imap_status(&state.pool, s.project_id, note).await {
                tracing::error!(error = %e, "cannot record IMAP status");
            }
        });
    }
    let n = set.len();
    while set.join_next().await.is_some() {}
    Ok(n)
}

/// Resolved tickets idle past the project's `auto_close_days` become closed.
pub async fn auto_close_once(state: &AppState) -> sd_db::Result<usize> {
    Ok(tickets::auto_close_resolved(&state.pool).await?.len())
}

async fn housekeeping_once(state: &AppState) -> sd_db::Result<()> {
    let closed = auto_close_once(state).await?;
    if closed > 0 {
        tracing::info!(closed, "auto-closed resolved tickets");
    }
    mailq::purge_finished(&state.pool, OUTBOX_RETENTION_DAYS).await?;
    Ok(())
}

fn every<F, Fut>(period: Duration, name: &'static str, f: F) -> JoinHandle<()>
where
    F: Fn() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = sd_db::Result<()>> + Send,
{
    tokio::spawn(async move {
        let mut tick = interval(period);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if let Err(e) = f().await {
                tracing::error!(job = name, error = %e, "background job failed");
            }
        }
    })
}

/// Start the jobs; abort the handles to stop them.
pub fn spawn(state: AppState) -> Vec<JoinHandle<()>> {
    let (a, b, c) = (state.clone(), state.clone(), state.clone());
    vec![
        every(state.config.mail.imap_poll_interval, "imap", move || {
            let s = a.clone();
            async move { poll_all(&s).await.map(drop) }
        }),
        every(OUTBOX_EVERY, "outbox", move || {
            let s = b.clone();
            async move { run_outbox_once(&s).await.map(drop) }
        }),
        every(HOUSEKEEPING_EVERY, "housekeeping", move || {
            let s = c.clone();
            async move { housekeeping_once(&s).await }
        }),
    ]
}
