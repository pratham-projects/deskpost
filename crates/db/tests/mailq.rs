mod common;

use common::*;
use sd_db::mailq::{self, NewInboundTicket};
use sd_db::{EmailLogStatus, Pool, Role, attachments};

fn inbound(project_id: sd_db::Id, mid: &str) -> NewInboundTicket {
    NewInboundTicket {
        project_id,
        client_email: "Ann@Example.com".into(),
        client_name: Some("Ann".into()),
        subject: "Help".into(),
        body: "please".into(),
        email_message_id: mid.into(),
        email_in_reply_to: None,
        email_references: None,
        email_from: Some("ann@example.com".into()),
        email_to: None,
        email_received_at: None,
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn inbound_ticket_is_atomic_and_deduped(pool: Pool) {
    let (_, pa, _, pb) = two_tenants(&pool).await;
    let first = mailq::create_inbound_ticket(&pool, &inbound(pa.id, "m1@x"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.ticket_number, 1);
    // Replay: nothing written, and the ticket number is not burned.
    assert!(
        mailq::create_inbound_ticket(&pool, &inbound(pa.id, "m1@x"))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(count(&pool, "tickets").await, 1);
    let second = mailq::create_inbound_ticket(&pool, &inbound(pa.id, "m2@x"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.ticket_number, 2);
    // Same Message-ID in another tenant is a different mail.
    assert!(
        mailq::create_inbound_ticket(&pool, &inbound(pb.id, "m1@x"))
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(count(&pool, "unread_tickets").await, 3);
    assert_eq!(count(&pool, "clients").await, 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn refs_find_messages_and_sent_auto_responses_per_project(pool: Pool) {
    let (_, pa, _, pb) = two_tenants(&pool).await;
    let t = mailq::create_inbound_ticket(&pool, &inbound(pa.id, "m1@x"))
        .await
        .unwrap()
        .unwrap();
    let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        mailq::find_ticket_by_refs(&pool, pa.id, &ids(&["zzz", "m1@x"]))
            .await
            .unwrap(),
        Some(t.ticket_id)
    );
    assert_eq!(
        mailq::find_ticket_by_refs(&pool, pb.id, &ids(&["m1@x"]))
            .await
            .unwrap(),
        None
    );

    let job = mailq::enqueue(&pool, pa.id, t.ticket_id, "created", "ann@example.com", "S")
        .await
        .unwrap()
        .unwrap();
    let claimed = mailq::claim_due(&pool, 10, 60).await.unwrap();
    assert_eq!(claimed[0].id, job);
    mailq::mark_sent(&pool, &claimed[0], "ar1@x").await.unwrap();
    // The client may reply to the auto-response before anything else.
    assert_eq!(
        mailq::find_ticket_by_refs(&pool, pa.id, &ids(&["ar1@x"]))
            .await
            .unwrap(),
        Some(t.ticket_id)
    );
    assert_eq!(
        mailq::find_ticket_by_refs(&pool, pb.id, &ids(&["ar1@x"]))
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        mailq::thread_ids(&pool, pa.id, t.ticket_id).await.unwrap(),
        ["m1@x", "ar1@x"]
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn outbox_leases_retries_and_gives_up(pool: Pool) {
    let (_, pa, _, _) = two_tenants(&pool).await;
    let t = mailq::create_inbound_ticket(&pool, &inbound(pa.id, "m1@x"))
        .await
        .unwrap()
        .unwrap();
    mailq::enqueue(&pool, pa.id, t.ticket_id, "created", "ann@example.com", "S")
        .await
        .unwrap();

    let job = mailq::claim_due(&pool, 10, 600).await.unwrap().remove(0);
    assert_eq!(job.attempts, 1);
    // Leased: a second worker sees nothing.
    assert!(mailq::claim_due(&pool, 10, 600).await.unwrap().is_empty());

    mailq::mark_failed(&pool, &job, "boom", Some(0))
        .await
        .unwrap();
    let job = mailq::claim_due(&pool, 10, 600).await.unwrap().remove(0);
    assert_eq!(job.attempts, 2);

    mailq::mark_failed(&pool, &job, "final", None)
        .await
        .unwrap();
    assert!(mailq::claim_due(&pool, 10, 600).await.unwrap().is_empty());
    let status: String = sqlx::query_scalar("SELECT status FROM mail_outbox")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "dead");
    let log = sqlx::query_as::<_, (EmailLogStatus, Option<String>)>(
        "SELECT status, error FROM email_logs",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(log, (EmailLogStatus::Failed, Some("final".into())));

    // Finished rows are purged after the retention window, pending ones never.
    assert_eq!(mailq::purge_finished(&pool, 30).await.unwrap(), 0);
    sqlx::query("UPDATE mail_outbox SET updated_at = now() - interval '31 days'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(mailq::purge_finished(&pool, 30).await.unwrap(), 1);
    assert_eq!(count(&pool, "email_logs").await, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn enqueue_is_bounded_per_address_and_per_project(pool: Pool) {
    let (_, pa, _, pb) = two_tenants(&pool).await;
    let t = mailq::create_inbound_ticket(&pool, &inbound(pa.id, "m1@x"))
        .await
        .unwrap()
        .unwrap();
    for i in 0..mailq::MAX_PER_ADDRESS_HOUR {
        assert!(
            mailq::enqueue(
                &pool,
                pa.id,
                t.ticket_id,
                "created",
                "Ann@example.com",
                &format!("s{i}")
            )
            .await
            .unwrap()
            .is_some()
        );
    }
    assert!(
        mailq::enqueue(&pool, pa.id, t.ticket_id, "created", "ann@EXAMPLE.com", "x")
            .await
            .unwrap()
            .is_none()
    );
    // Another address, and the same address in another tenant, are unaffected.
    assert!(
        mailq::enqueue(&pool, pa.id, t.ticket_id, "created", "bob@example.com", "x")
            .await
            .unwrap()
            .is_some()
    );
    let tb = mailq::create_inbound_ticket(&pool, &inbound(pb.id, "m9@x"))
        .await
        .unwrap()
        .unwrap();
    assert!(
        mailq::enqueue(
            &pool,
            pb.id,
            tb.ticket_id,
            "created",
            "ann@example.com",
            "x"
        )
        .await
        .unwrap()
        .is_some()
    );
    // A ticket of another project can never be queued (composite FK).
    assert!(
        mailq::enqueue(&pool, pb.id, t.ticket_id, "created", "c@example.com", "x")
            .await
            .is_err()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn attachment_delete_needs_uploader_or_admin_and_is_tenant_scoped(pool: Pool) {
    let (a, pa, b, _) = two_tenants(&pool).await;
    let agent = user(&pool, "agent@example.com").await;
    let other = user(&pool, "other@example.com").await;
    add_member(&pool, a.id, pa.id, agent.id, Role::Agent).await;
    add_member(&pool, a.id, pa.id, other.id, Role::Agent).await;
    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;
    let mk = |key: &str| sd_db::attachments::NewAttachment {
        filename: "f.txt".into(),
        mime_type: "text/plain".into(),
        size_bytes: 3,
        storage_key: key.into(),
        message_id: None,
    };
    let sd_db::attachments::Stored::Ok(att) = attachments::create(&pool, agent.id, t.id, &mk("k1"))
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        mailq::delete_attachment(&pool, b.id, att.id).await.unwrap(),
        None,
        "outsider"
    );
    assert_eq!(
        mailq::delete_attachment(&pool, other.id, att.id)
            .await
            .unwrap(),
        None,
        "not uploader"
    );
    assert_eq!(
        mailq::delete_attachment(&pool, agent.id, att.id)
            .await
            .unwrap()
            .as_deref(),
        Some("k1")
    );
    let sd_db::attachments::Stored::Ok(att) = attachments::create(&pool, agent.id, t.id, &mk("k2"))
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        mailq::delete_attachment(&pool, a.id, att.id)
            .await
            .unwrap()
            .as_deref(),
        Some("k2"),
        "admin"
    );
    // Linked uploads are kept by the rollback helper.
    let sd_db::attachments::Stored::Ok(att) = attachments::create(&pool, agent.id, t.id, &mk("k3"))
        .await
        .unwrap()
    else {
        panic!()
    };
    mailq::delete_attachments(&pool, t.id, &[att.id])
        .await
        .unwrap();
    assert!(
        mailq::attachment_system(&pool, att.id)
            .await
            .unwrap()
            .is_none()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn imap_status_round_trips(pool: Pool) {
    let (_, pa, _, _) = two_tenants(&pool).await;
    mailq::set_imap_status(&pool, pa.id, Some("login failed"))
        .await
        .unwrap();
    let (err, at) = mailq::imap_status(&pool, pa.id).await.unwrap().unwrap();
    assert_eq!(err.as_deref(), Some("login failed"));
    assert!(at.is_some());
    mailq::set_imap_status(&pool, pa.id, None).await.unwrap();
    assert_eq!(
        mailq::imap_status(&pool, pa.id).await.unwrap().unwrap().0,
        None
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn crash_looping_jobs_are_dead_lettered_not_reclaimed(pool: Pool) {
    let (_, pa, _, _) = two_tenants(&pool).await;
    let t = mailq::create_inbound_ticket(&pool, &inbound(pa.id, "m1@x"))
        .await
        .unwrap()
        .unwrap();
    mailq::enqueue(&pool, pa.id, t.ticket_id, "created", "ann@example.com", "S")
        .await
        .unwrap();
    // Claimed five times, each time the worker died before reporting: leases ran out.
    for _ in 0..5 {
        mailq::claim_due(&pool, 10, 600).await.unwrap();
        sqlx::query("UPDATE mail_outbox SET next_attempt_at = now()")
            .execute(&pool)
            .await
            .unwrap();
    }
    assert_eq!(mailq::dead_letter_exhausted(&pool, 5).await.unwrap(), 1);
    assert!(mailq::claim_due(&pool, 10, 600).await.unwrap().is_empty());
    let (job, log): (String, String) = sqlx::query_as(
        "SELECT o.status, l.status FROM mail_outbox o JOIN email_logs l ON l.id = o.email_log_id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((job.as_str(), log.as_str()), ("dead", "failed"));
    // nothing left to give up
    assert_eq!(mailq::dead_letter_exhausted(&pool, 5).await.unwrap(), 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_job_still_inside_its_lease_is_not_dead_lettered(pool: Pool) {
    let (_, pa, _, _) = two_tenants(&pool).await;
    let t = mailq::create_inbound_ticket(&pool, &inbound(pa.id, "m1@x"))
        .await
        .unwrap()
        .unwrap();
    mailq::enqueue(&pool, pa.id, t.ticket_id, "created", "ann@example.com", "S")
        .await
        .unwrap();
    sqlx::query("UPDATE mail_outbox SET attempts = 5")
        .execute(&pool)
        .await
        .unwrap();
    mailq::claim_due(&pool, 10, 600).await.unwrap();
    // the 6th claim is leased and in flight
    assert_eq!(mailq::dead_letter_exhausted(&pool, 5).await.unwrap(), 0);
}
