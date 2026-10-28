mod common;
mod mailkit;

use std::time::Duration;

use api::mail_out::{enqueue_auto_response, redact};
use api::workers::{auto_close_once, poll_all, poll_project, spawn};
use mailkit::*;
use sd_db::{EmailResponseStatus, Role, email_responses, mailq, projects};
use sd_mail::testing::{FakeImap, FakeSmtp};
use sqlx::PgPool;

const IMAP_PW: &str = "imap-secret-pw";

async fn project_with_imap(k: &Kit, owner: &str, slug: &str, imap: &FakeImap) -> sd_db::Id {
    let user = k.t.user(owner, Role::Agent).await;
    let p = k.t.project(user.id, slug).await;
    k.configure_mail(
        p.id,
        closed_port(),
        Some((imap.port(), "imapuser", IMAP_PW)),
    )
    .await;
    p.id
}

async fn cursor(k: &Kit, p: sd_db::Id) -> (Option<i64>, i64) {
    sqlx::query_as("SELECT imap_uid_validity, imap_last_uid FROM projects WHERE id = $1")
        .bind(p)
        .fetch_one(&k.t.pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn polls_threads_dedupes_and_survives_hostile_mail(pool: PgPool) {
    let k = Kit::build(pool, false, |c| c.mail.max_request_bytes = 20_000).await;
    let imap = FakeImap::start("imapuser", IMAP_PW).await;
    let p = project_with_imap(&k, "o@example.com", "acme", &imap).await;

    imap.push(&customer_mail("<m1@mail.example>", "Printer", "first", &[]));
    imap.push(b"this is not mail at all \x00\xff\xfe");
    imap.push(&customer_mail(
        "<m2@mail.example>",
        "Re: Printer",
        "second",
        &[("In-Reply-To", "<m1@mail.example>")],
    ));
    imap.push(&customer_mail(
        "<m1@mail.example>",
        "Printer",
        "first again",
        &[],
    ));
    imap.push(&customer_mail(
        "<huge@mail.example>",
        "Huge",
        &"x".repeat(40_000),
        &[],
    ));
    imap.push(&customer_mail(
        "<bulk@mail.example>",
        "Newsletter",
        "buy",
        &[("Precedence", "bulk")],
    ));
    imap.push(&customer_mail(
        "<m3@mail.example>",
        "Other",
        "third",
        &[("From", "bob@example.com")],
    ));
    let last = imap.push(&customer_mail(
        "<auto@mail.example>",
        "Away",
        "ooo",
        &[("Auto-Submitted", "auto-replied")],
    ));

    assert_eq!(poll_all(&k.t.state).await.unwrap(), 1);
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT t.ticket_number, (SELECT count(*) FROM messages m WHERE m.ticket_id = t.id)::text FROM tickets t ORDER BY 1").fetch_all(&k.t.pool).await.unwrap();
    assert_eq!(
        rows,
        [(1, "2".into()), (2, "1".into())],
        "m1+m2 threaded, m3 new; garbage, dup, huge, bulk and auto ignored"
    );
    let (validity, uid) = cursor(&k, p).await;
    assert_eq!((validity.is_some(), uid), (true, i64::from(last)));
    let (err, at) = mailq::imap_status(&k.t.pool, p).await.unwrap().unwrap();
    assert_eq!(err, None);
    assert!(at.is_some());

    // A second round has nothing new to do and does not re-deliver the last mail.
    poll_all(&k.t.state).await.unwrap();
    assert_eq!(
        (k.count("tickets").await, k.count("messages").await),
        (2, 3)
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn drains_a_backlog_in_batches_of_fifty(pool: PgPool) {
    let k = Kit::new(pool).await;
    let imap = FakeImap::start("imapuser", IMAP_PW).await;
    let p = project_with_imap(&k, "o@example.com", "acme", &imap).await;
    for i in 0..120 {
        imap.push(&customer_mail(
            &format!("<b{i}@mail.example>"),
            &format!("Subject {i}"),
            "x",
            &[],
        ));
    }
    let s = projects::mail_settings(&k.t.pool, p)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(poll_project(&k.t.state, &s).await, Ok(120));
    assert_eq!(cursor(&k, p).await.1, 120);
    assert_eq!(k.count("tickets").await, 120);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn uidvalidity_change_rescans_without_duplicates(pool: PgPool) {
    let k = Kit::new(pool).await;
    let imap = FakeImap::start("imapuser", IMAP_PW).await;
    let p = project_with_imap(&k, "o@example.com", "acme", &imap).await;
    imap.push(&customer_mail("<v1@mail.example>", "One", "a", &[]));
    imap.push(&customer_mail("<v2@mail.example>", "Two", "b", &[]));
    poll_all(&k.t.state).await.unwrap();
    let (v1, _) = cursor(&k, p).await;

    imap.reset_mailbox(777);
    imap.push(&customer_mail("<v1@mail.example>", "One", "a", &[]));
    imap.push(&customer_mail("<v2@mail.example>", "Two", "b", &[]));
    imap.push(&customer_mail("<v3@mail.example>", "Three", "c", &[]));
    poll_all(&k.t.state).await.unwrap();
    assert_eq!(
        k.count("tickets").await,
        3,
        "already-seen mail is deduped by Message-ID"
    );
    assert_eq!(cursor(&k, p).await, (Some(777), 3));
    assert_ne!(v1, Some(777));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn one_failing_project_is_recorded_and_does_not_stop_the_rest(pool: PgPool) {
    let k = Kit::build(pool, false, |c| c.mail.imap_concurrency = 1).await;
    let good = FakeImap::start("imapuser", IMAP_PW).await;
    let bad = FakeImap::start("imapuser", "a-different-password").await;
    let a = project_with_imap(&k, "a@example.com", "aaa", &bad).await;
    let b = project_with_imap(&k, "b@example.com", "bbb", &good).await;
    bad.push(&customer_mail("<x@mail.example>", "Never read", "x", &[]));
    good.push(&customer_mail("<y@mail.example>", "Read", "y", &[]));

    assert_eq!(poll_all(&k.t.state).await.unwrap(), 2);
    let (err, at) = mailq::imap_status(&k.t.pool, a).await.unwrap().unwrap();
    let err = err.expect("failure recorded");
    assert!(at.is_some());
    assert!(
        !err.contains(IMAP_PW) && !err.contains("a-different-password"),
        "{err}"
    );
    assert_eq!(cursor(&k, a).await, (None, 0));
    assert_eq!(
        mailq::imap_status(&k.t.pool, b).await.unwrap().unwrap().0,
        None
    );
    // The mail landed in the project it was fetched for, and only there.
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT p.slug, t.subject FROM tickets t JOIN projects p ON p.id = t.project_id",
    )
    .fetch_all(&k.t.pool)
    .await
    .unwrap();
    assert_eq!(rows, [("bbb".to_string(), "Read".to_string())]);

    // Once the password is right the failure is cleared and the backlog is read.
    sqlx::query("UPDATE projects SET imap_password_encrypted = $2 WHERE id = $1")
        .bind(a)
        .bind(
            k.t.state
                .secrets
                .seal("a-different-password", &api::mail_out::imap_aad(a)),
        )
        .execute(&k.t.pool)
        .await
        .unwrap();
    poll_all(&k.t.state).await.unwrap();
    assert_eq!(
        mailq::imap_status(&k.t.pool, a).await.unwrap().unwrap().0,
        None
    );
    assert_eq!(k.count("tickets").await, 2);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn unreachable_or_misconfigured_imap_is_an_error_not_a_panic(pool: PgPool) {
    let k = Kit::new(pool).await;
    let imap = FakeImap::start("imapuser", IMAP_PW).await;
    let p = project_with_imap(&k, "o@example.com", "acme", &imap).await;
    sqlx::query("UPDATE projects SET imap_port = $2 WHERE id = $1")
        .bind(p)
        .bind(i32::from(closed_port()))
        .execute(&k.t.pool)
        .await
        .unwrap();
    let s = projects::mail_settings(&k.t.pool, p)
        .await
        .unwrap()
        .unwrap();
    assert!(poll_project(&k.t.state, &s).await.is_err());
    // A password sealed for another project cannot be opened: reported, not used.
    let other = k.t.user("x@example.com", Role::Agent).await;
    let q = k.t.project(other.id, "other").await;
    sqlx::query("UPDATE projects SET imap_password_encrypted = (SELECT imap_password_encrypted FROM projects WHERE id = $1) WHERE id = $2").bind(p).bind(q.id).execute(&k.t.pool).await.unwrap();
    let mut s2 = projects::mail_settings(&k.t.pool, q.id)
        .await
        .unwrap()
        .unwrap();
    s2.imap_host = Some("127.0.0.1".into());
    s2.imap_username = Some("imapuser".into());
    assert_eq!(
        poll_project(&k.t.state, &s2).await,
        Err("cannot open the stored IMAP password".into())
    );
    assert_eq!(redact("x", &[]), "x");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn auto_close_only_closes_old_resolved_tickets_with_the_job_enabled(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (a, p1, old, _) = k.tenant("a@example.com", "aaa").await;
    let (_, p2, never, _) = k.tenant("b@example.com", "bbb").await;
    let fresh = k.ticket(&a, &p1, "fresh@example.com").await;
    let open = k.ticket(&a, &p1, "open@example.com").await;
    sqlx::query("UPDATE projects SET auto_close_days = 0 WHERE id = $1")
        .bind(p2.id)
        .execute(&k.t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE tickets SET status = 'resolved', updated_at = now() - interval '10 days' WHERE id = ANY($1)").bind([old.id, never.id]).execute(&k.t.pool).await.unwrap();
    sqlx::query("UPDATE tickets SET status = 'resolved' WHERE id = $1")
        .bind(fresh.id)
        .execute(&k.t.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE tickets SET updated_at = now() - interval '30 days' WHERE id = $1")
        .bind(open.id)
        .execute(&k.t.pool)
        .await
        .unwrap();

    assert_eq!(auto_close_once(&k.t.state).await.unwrap(), 1);
    let rows: Vec<(sd_db::Id, String, bool)> =
        sqlx::query_as("SELECT id, status, closed_at IS NOT NULL FROM tickets")
            .fetch_all(&k.t.pool)
            .await
            .unwrap();
    let get = |id| {
        rows.iter()
            .find(|r| r.0 == id)
            .map(|r| (r.1.as_str(), r.2))
            .unwrap()
    };
    assert_eq!(get(old.id), ("closed", true));
    assert_eq!(
        get(never.id),
        ("resolved", false),
        "auto_close_days = 0 means never"
    );
    assert_eq!(get(fresh.id), ("resolved", false));
    assert_eq!(get(open.id), ("pending", false));
    assert_eq!(auto_close_once(&k.t.state).await.unwrap(), 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn spawned_jobs_send_the_queue(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (owner, project, ticket, _) = k.tenant("o@example.com", "acme").await;
    let smtp = FakeSmtp::start().await;
    k.configure_mail(project.id, smtp.port(), None).await;
    email_responses::create(
        &k.t.pool,
        owner.id,
        project.id,
        EmailResponseStatus::Created,
        "",
        "Hello",
        true,
    )
    .await
    .unwrap()
    .unwrap();
    enqueue_auto_response(
        &k.t.pool,
        project.id,
        ticket.id,
        EmailResponseStatus::Created,
    )
    .await
    .unwrap();

    let jobs = spawn(k.t.state.clone());
    assert_eq!(jobs.len(), 3);
    let mut sent = false;
    for _ in 0..50 {
        if !smtp.messages().is_empty() {
            sent = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    jobs.iter().for_each(|j| j.abort());
    assert!(sent, "outbox worker delivered the queued auto-response");
}
