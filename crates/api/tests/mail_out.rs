mod common;
mod mailkit;

use api::error::ApiError;
use api::mail_out::*;
use mailkit::*;
use proptest::prelude::*;
use sd_db::{EmailResponseStatus, email_responses};
use sd_mail::{Limits, parse_inbound};
use sqlx::PgPool;

fn reply(project: sd_db::Id, ticket: sd_db::Id, body: &str) -> ReplyMail {
    ReplyMail {
        project_id: project,
        ticket_id: ticket,
        sent_by: None,
        body: body.into(),
        attachment_ids: vec![],
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reply_has_threading_headers_credentials_and_a_log(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (_, project, ticket, _) = k.tenant("owner@example.com", "acme").await;
    let smtp = sd_mail::testing::FakeSmtp::start_requiring_auth("u", "smtp-secret-pw").await;
    k.configure_mail(project.id, smtp.port(), None).await;
    sqlx::query("UPDATE messages SET email_message_id = 'client-1@mail.example'")
        .execute(&k.t.pool)
        .await
        .unwrap();

    let first =
        k.t.state
            .mailer
            .send_reply(reply(project.id, ticket.id, "We are on it"))
            .await
            .unwrap();
    assert_eq!(first.in_reply_to.as_deref(), Some("client-1@mail.example"));
    assert_eq!(first.to, "client@example.com");
    assert!(
        first
            .email_message_id
            .starts_with(&format!("ticket-{}-reply-", ticket.id))
    );
    assert!(first.email_message_id.ends_with("@acme.test"));
    assert!(!first.email_message_id.contains('<'));

    let got = &smtp.messages()[0];
    assert_eq!(got.auth, Some(("u".into(), "smtp-secret-pw".into())));
    assert_eq!(got.rcpt_to, ["client@example.com"]);
    let m = parse_inbound(&got.data, &Limits::default()).unwrap();
    assert_eq!(m.message_id, first.email_message_id);
    assert_eq!(m.in_reply_to.as_deref(), Some("client-1@mail.example"));
    assert_eq!(m.references, ["client-1@mail.example"]);
    assert_eq!(
        m.subject,
        format!(
            "Re: Project acme Support - [Ticket #{}]",
            ticket.ticket_number
        )
    );
    assert_eq!(m.from.as_deref(), Some(MAILBOX));
    assert_eq!(m.text.as_deref().map(str::trim), Some("We are on it"));

    // The next reply continues the chain from the one before (known through the email log).
    let second =
        k.t.state
            .mailer
            .send_reply(reply(project.id, ticket.id, "Update"))
            .await
            .unwrap();
    assert_eq!(
        second.in_reply_to.as_deref(),
        Some(first.email_message_id.as_str())
    );
    assert_eq!(
        second.references.as_deref(),
        Some(format!("client-1@mail.example {}", first.email_message_id).as_str())
    );

    let logs: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT status, smtp_message_id FROM email_logs ORDER BY created_at")
            .fetch_all(&k.t.pool)
            .await
            .unwrap();
    assert_eq!(
        logs,
        [
            ("sent".into(), Some(first.email_message_id)),
            ("sent".into(), Some(second.email_message_id))
        ]
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn hostile_project_name_cannot_inject_headers(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (_, project, ticket, _) = k.tenant("owner@example.com", "acme").await;
    let smtp = sd_mail::testing::FakeSmtp::start().await;
    k.configure_mail(project.id, smtp.port(), None).await;
    sqlx::query("UPDATE projects SET name = E'Acme\\r\\nBcc: evil@attacker.test'")
        .execute(&k.t.pool)
        .await
        .unwrap();
    k.t.state
        .mailer
        .send_reply(reply(project.id, ticket.id, "hi"))
        .await
        .unwrap();
    let got = &smtp.messages()[0];
    assert_eq!(got.rcpt_to, ["client@example.com"]);
    assert!(!String::from_utf8_lossy(&got.data).contains("\r\nBcc:"));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reply_errors_are_typed_and_leak_no_password(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (_, project, ticket, _) = k.tenant("owner@example.com", "acme").await;
    let none =
        k.t.state
            .mailer
            .send_reply(reply(project.id, ticket.id, "x"))
            .await;
    assert_eq!(none, Err(MailError::NotConfigured));
    k.configure_mail(project.id, closed_port(), None).await;
    let err =
        k.t.state
            .mailer
            .send_reply(reply(project.id, ticket.id, "x"))
            .await
            .unwrap_err();
    let MailError::Delivery(msg) = &err else {
        panic!("{err:?}")
    };
    assert!(!msg.contains("smtp-secret-pw"));
    assert_eq!(ApiError::from(err.clone()), ApiError::MailDeliveryFailed);
    assert_eq!(
        ApiError::from(MailError::NotConfigured).status().as_u16(),
        422
    );
    // Another project's ticket id never resolves (the ticket is looked up inside the project).
    let (_, other, _, _) = k.tenant("o2@example.com", "other").await;
    k.configure_mail(other.id, closed_port(), None).await;
    let e =
        k.t.state
            .mailer
            .send_reply(reply(other.id, ticket.id, "x"))
            .await
            .unwrap_err();
    assert!(matches!(e, MailError::Internal(_)), "{e:?}");
    // A sealed password that cannot be opened (wrong project binding) is an internal error.
    sqlx::query("UPDATE projects SET smtp_password_encrypted = (SELECT smtp_password_encrypted FROM projects WHERE id = $1) WHERE id = $2")
        .bind(project.id).bind(other.id).execute(&k.t.pool).await.unwrap();
    let e =
        k.t.state
            .mailer
            .send_reply(reply(other.id, ticket.id, "x"))
            .await
            .unwrap_err();
    assert!(matches!(e, MailError::Internal(_)));
}

async fn response(
    k: &Kit,
    owner: sd_db::Id,
    project: sd_db::Id,
    kind: EmailResponseStatus,
    subject: &str,
    enabled: bool,
) {
    email_responses::create(
        &k.t.pool,
        owner,
        project,
        kind,
        subject,
        "Thanks, we got your request.",
        enabled,
    )
    .await
    .unwrap()
    .unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn created_auto_response_is_queued_then_sent_with_tagged_subject(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (owner, project, ticket, _) = k.tenant("owner@example.com", "acme").await;
    let smtp = sd_mail::testing::FakeSmtp::start().await;
    k.configure_mail(project.id, smtp.port(), None).await;

    // Nothing configured: nothing queued.
    assert!(
        !enqueue_auto_response(
            &k.t.pool,
            project.id,
            ticket.id,
            EmailResponseStatus::Created
        )
        .await
        .unwrap()
    );
    response(
        &k,
        owner.id,
        project.id,
        EmailResponseStatus::Created,
        "We got it",
        true,
    )
    .await;
    assert!(
        enqueue_auto_response(
            &k.t.pool,
            project.id,
            ticket.id,
            EmailResponseStatus::Created
        )
        .await
        .unwrap()
    );
    let pending: String = sqlx::query_scalar("SELECT status FROM email_logs")
        .fetch_one(&k.t.pool)
        .await
        .unwrap();
    assert_eq!(pending, "pending");

    assert_eq!(run_outbox_once(&k.t.state).await.unwrap(), 1);
    assert_eq!(
        run_outbox_once(&k.t.state).await.unwrap(),
        0,
        "sent jobs are not sent again"
    );
    let m = parse_inbound(&smtp.messages()[0].data, &Limits::default()).unwrap();
    assert_eq!(
        m.subject,
        format!("We got it [Ticket #{}]", ticket.ticket_number)
    );
    assert_eq!(m.message_id, format!("ticket-{}@acme.test", ticket.id));
    let text = m.text.unwrap();
    assert!(
        text.contains("Thanks, we got your request.") && !text.contains("It is burning"),
        "{text}"
    );
    let log: (String, Option<String>) =
        sqlx::query_as("SELECT status, smtp_message_id FROM email_logs")
            .fetch_one(&k.t.pool)
            .await
            .unwrap();
    assert_eq!(log, ("sent".into(), Some(m.message_id)));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn disabled_response_is_not_queued_and_removed_response_is_not_sent(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (owner, project, ticket, _) = k.tenant("owner@example.com", "acme").await;
    let smtp = sd_mail::testing::FakeSmtp::start().await;
    k.configure_mail(project.id, smtp.port(), None).await;
    response(
        &k,
        owner.id,
        project.id,
        EmailResponseStatus::Resolved,
        "",
        false,
    )
    .await;
    assert!(
        !enqueue_auto_response(
            &k.t.pool,
            project.id,
            ticket.id,
            EmailResponseStatus::Resolved
        )
        .await
        .unwrap()
    );
    assert_eq!(k.count("mail_outbox").await, 0);

    sqlx::query("UPDATE email_responses SET is_enabled = true")
        .execute(&k.t.pool)
        .await
        .unwrap();
    assert!(
        enqueue_auto_response(
            &k.t.pool,
            project.id,
            ticket.id,
            EmailResponseStatus::Resolved
        )
        .await
        .unwrap()
    );
    sqlx::query("DELETE FROM email_responses")
        .execute(&k.t.pool)
        .await
        .unwrap();
    run_outbox_once(&k.t.state).await.unwrap();
    assert!(smtp.messages().is_empty());
    let (st, err): (String, Option<String>) =
        sqlx::query_as("SELECT status, error FROM email_logs")
            .fetch_one(&k.t.pool)
            .await
            .unwrap();
    assert_eq!(st, "failed");
    assert!(err.unwrap().contains("disabled"));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn resolved_response_threads_onto_the_conversation(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (owner, project, ticket, _) = k.tenant("owner@example.com", "acme").await;
    let smtp = sd_mail::testing::FakeSmtp::start().await;
    k.configure_mail(project.id, smtp.port(), None).await;
    sqlx::query("UPDATE messages SET email_message_id = 'c1@mail.example'")
        .execute(&k.t.pool)
        .await
        .unwrap();
    response(
        &k,
        owner.id,
        project.id,
        EmailResponseStatus::Resolved,
        "",
        true,
    )
    .await;
    enqueue_auto_response(
        &k.t.pool,
        project.id,
        ticket.id,
        EmailResponseStatus::Resolved,
    )
    .await
    .unwrap();
    run_outbox_once(&k.t.state).await.unwrap();
    let m = parse_inbound(&smtp.messages()[0].data, &Limits::default()).unwrap();
    assert_eq!(m.in_reply_to.as_deref(), Some("c1@mail.example"));
    assert_eq!(
        m.subject,
        format!("Project acme Support - [Ticket #{}]", ticket.ticket_number)
    );
    assert!(
        m.message_id
            .starts_with(&format!("ticket-{}-reply-", ticket.id))
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn failures_retry_with_backoff_and_then_give_up(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (owner, project, ticket, _) = k.tenant("owner@example.com", "acme").await;
    k.configure_mail(project.id, closed_port(), None).await;
    response(
        &k,
        owner.id,
        project.id,
        EmailResponseStatus::Created,
        "",
        true,
    )
    .await;
    enqueue_auto_response(
        &k.t.pool,
        project.id,
        ticket.id,
        EmailResponseStatus::Created,
    )
    .await
    .unwrap();

    for attempt in 1..MAX_ATTEMPTS {
        assert_eq!(run_outbox_once(&k.t.state).await.unwrap(), 1);
        let (st, n, wait): (String, i32, f64) = sqlx::query_as(
            "SELECT status, attempts, extract(epoch FROM next_attempt_at - now())::float8 FROM mail_outbox",
        ).fetch_one(&k.t.pool).await.unwrap();
        assert_eq!((st.as_str(), n), ("pending", attempt));
        let want = backoff_secs(attempt) as f64;
        assert!(
            (want - 5.0..=want).contains(&wait),
            "attempt {attempt}: wait {wait}, want ~{want}"
        );
        // Not due yet: the queue leaves it alone.
        assert_eq!(run_outbox_once(&k.t.state).await.unwrap(), 0);
        sqlx::query("UPDATE mail_outbox SET next_attempt_at = now()")
            .execute(&k.t.pool)
            .await
            .unwrap();
    }
    assert_eq!(run_outbox_once(&k.t.state).await.unwrap(), 1);
    let (st, n, last): (String, i32, Option<String>) =
        sqlx::query_as("SELECT status, attempts, last_error FROM mail_outbox")
            .fetch_one(&k.t.pool)
            .await
            .unwrap();
    assert_eq!((st.as_str(), n), ("dead", MAX_ATTEMPTS));
    assert!(!last.unwrap().contains("smtp-secret-pw"));
    let (lst, lerr): (String, Option<String>) =
        sqlx::query_as("SELECT status, error FROM email_logs")
            .fetch_one(&k.t.pool)
            .await
            .unwrap();
    assert_eq!(lst, "failed");
    assert!(lerr.is_some());
    sqlx::query("UPDATE mail_outbox SET next_attempt_at = now()")
        .execute(&k.t.pool)
        .await
        .unwrap();
    assert_eq!(
        run_outbox_once(&k.t.state).await.unwrap(),
        0,
        "dead jobs stay dead"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn a_recovered_server_gets_the_queued_mail(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (owner, project, ticket, _) = k.tenant("owner@example.com", "acme").await;
    k.configure_mail(project.id, closed_port(), None).await;
    response(
        &k,
        owner.id,
        project.id,
        EmailResponseStatus::Created,
        "",
        true,
    )
    .await;
    enqueue_auto_response(
        &k.t.pool,
        project.id,
        ticket.id,
        EmailResponseStatus::Created,
    )
    .await
    .unwrap();
    run_outbox_once(&k.t.state).await.unwrap();
    let smtp = sd_mail::testing::FakeSmtp::start_requiring_auth("u", "smtp-secret-pw").await;
    k.configure_mail(project.id, smtp.port(), None).await;
    sqlx::query("UPDATE mail_outbox SET next_attempt_at = now()")
        .execute(&k.t.pool)
        .await
        .unwrap();
    run_outbox_once(&k.t.state).await.unwrap();
    assert_eq!(smtp.messages().len(), 1);
    let (st, n): (String, i32) = sqlx::query_as("SELECT status, attempts FROM mail_outbox")
        .fetch_one(&k.t.pool)
        .await
        .unwrap();
    assert_eq!((st.as_str(), n), ("sent", 2));
    let st: String = sqlx::query_scalar("SELECT status FROM email_logs")
        .fetch_one(&k.t.pool)
        .await
        .unwrap();
    assert_eq!(st, "sent");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn unconfigured_smtp_is_given_up_at_once(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (owner, project, ticket, _) = k.tenant("owner@example.com", "acme").await;
    response(
        &k,
        owner.id,
        project.id,
        EmailResponseStatus::Created,
        "",
        true,
    )
    .await;
    enqueue_auto_response(
        &k.t.pool,
        project.id,
        ticket.id,
        EmailResponseStatus::Created,
    )
    .await
    .unwrap();
    run_outbox_once(&k.t.state).await.unwrap();
    let (st, n): (String, i32) = sqlx::query_as("SELECT status, attempts FROM mail_outbox")
        .fetch_one(&k.t.pool)
        .await
        .unwrap();
    assert_eq!((st.as_str(), n), ("dead", 1));
}

#[test]
fn backoff_grows_and_is_capped() {
    assert_eq!(
        (1..=6).map(backoff_secs).collect::<Vec<_>>(),
        [30, 120, 480, 1920, 3600, 3600]
    );
    assert_eq!(backoff_secs(0), 30);
}

#[test]
fn tls_follows_port_unless_plaintext() {
    use sd_mail::TlsMode::*;
    assert_eq!(tls_for(465, false, 465), Implicit);
    assert_eq!(tls_for(587, false, 465), StartTls);
    assert_eq!(tls_for(465, true, 465), None);
}

#[test]
fn redact_strips_secrets_control_chars_and_length() {
    let r = redact("login hunter2 failed\r\nline2", &["hunter2", ""]);
    assert_eq!(r, "login *** failed  line2");
    assert_eq!(redact(&"x".repeat(1000), &[]).len(), 300);
}

proptest! {
    #[test]
    fn backoff_is_monotonic_and_bounded(a in 0i32..1000) {
        prop_assert!(backoff_secs(a) >= 30 && backoff_secs(a) <= 3600);
        prop_assert!(backoff_secs(a + 1) >= backoff_secs(a));
    }

    #[test]
    fn redact_never_keeps_the_secret(secret in "[a-zA-Z0-9]{4,20}", pre in "[ a-z]{0,20}", post in "[ a-z]{0,20}") {
        let out = redact(&format!("{pre}{secret}{post}"), &[&secret]);
        prop_assert!(!out.contains(&secret) || pre.contains(&secret) || post.contains(&secret));
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn auto_response_quotes_only_a_short_excerpt(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (owner, project, ticket, _) = k.tenant("owner@example.com", "acme").await;
    let smtp = sd_mail::testing::FakeSmtp::start().await;
    k.configure_mail(project.id, smtp.port(), None).await;
    sqlx::query("UPDATE tickets SET description = repeat('a', 5000)")
        .execute(&k.t.pool)
        .await
        .unwrap();
    response(
        &k,
        owner.id,
        project.id,
        EmailResponseStatus::Created,
        "",
        true,
    )
    .await;
    enqueue_auto_response(
        &k.t.pool,
        project.id,
        ticket.id,
        EmailResponseStatus::Created,
    )
    .await
    .unwrap();
    run_outbox_once(&k.t.state).await.unwrap();
    let text = parse_inbound(&smtp.messages()[0].data, &Limits::default())
        .unwrap()
        .text
        .unwrap();
    assert!(text.matches('a').count() < 400, "{}", text.len());
}
