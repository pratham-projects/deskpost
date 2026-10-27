mod common;

use common::*;
use sd_db::email_logs::NewEmailLog;
use sd_db::{
    EmailLogStatus, EmailResponseStatus as Status, Page, Pool, Role, constraint, email_logs,
    email_responses, new_id, templates, unread, viewers,
};

#[sqlx::test(migrations = "./migrations")]
async fn templates_are_project_scoped_with_manage_permission(pool: Pool) {
    let (a, pa, b, pb) = two_tenants(&pool).await;
    let agent = user(&pool, "agent@example.com").await;
    add_member(&pool, a.id, pa.id, agent.id, Role::Agent).await;
    let page = Page::new(50, 0);

    // Plain agents can read but not write.
    assert!(
        templates::create(&pool, agent.id, pa.id, "Hi", "Hello")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        templates::create(&pool, b.id, pa.id, "Hi", "Hello")
            .await
            .unwrap()
            .is_none(),
        "outsider"
    );
    let t = templates::create(&pool, a.id, pa.id, "Greeting", "Hello there")
        .await
        .unwrap()
        .unwrap();
    templates::create(&pool, a.id, pa.id, "apology", "Sorry about 100% of it")
        .await
        .unwrap()
        .unwrap();
    templates::create(&pool, b.id, pb.id, "Other tenant", "secret")
        .await
        .unwrap()
        .unwrap();

    let (all, total) = templates::list(&pool, agent.id, pa.id, None, page)
        .await
        .unwrap();
    assert_eq!(
        (
            total,
            all.iter().map(|t| t.title.as_str()).collect::<Vec<_>>()
        ),
        (2, vec!["apology", "Greeting"])
    );
    assert_eq!(
        templates::list(&pool, agent.id, pa.id, Some("HELLO"), page)
            .await
            .unwrap()
            .1,
        1
    );
    assert_eq!(
        templates::list(&pool, agent.id, pa.id, Some("100%"), page)
            .await
            .unwrap()
            .1,
        1
    );
    assert_eq!(
        templates::list(&pool, agent.id, pa.id, Some("%"), page)
            .await
            .unwrap()
            .1,
        1,
        "% is literal"
    );
    assert_eq!(
        templates::list(&pool, agent.id, pa.id, Some(""), page)
            .await
            .unwrap()
            .1,
        2
    );
    assert_eq!(
        templates::list(&pool, agent.id, pa.id, None, Page::new(1, 1))
            .await
            .unwrap()
            .0
            .len(),
        1
    );
    assert_eq!(
        templates::list(&pool, b.id, pa.id, None, page)
            .await
            .unwrap()
            .1,
        0,
        "foreign project is empty"
    );
    assert!(
        templates::get(&pool, agent.id, t.id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(templates::get(&pool, b.id, t.id).await.unwrap().is_none());

    assert!(
        templates::update(&pool, agent.id, t.id, Some("x"), None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(!templates::delete(&pool, agent.id, t.id).await.unwrap());
    sd_db::members::update(
        &pool,
        a.id,
        pa.id,
        agent.id,
        &sd_db::members::MemberPatch {
            can_manage_templates: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    let up = templates::update(&pool, agent.id, t.id, Some("Welcome"), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (up.title.as_str(), up.body.as_str()),
        ("Welcome", "Hello there")
    );
    let up = templates::update(&pool, agent.id, t.id, None, Some("New body"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (up.title.as_str(), up.body.as_str()),
        ("Welcome", "New body")
    );
    assert!(
        templates::update(&pool, b.id, t.id, Some("pwn"), None)
            .await
            .unwrap()
            .is_none()
    );
    assert!(!templates::delete(&pool, b.id, t.id).await.unwrap());
    assert!(templates::delete(&pool, agent.id, t.id).await.unwrap());
    assert!(!templates::delete(&pool, agent.id, t.id).await.unwrap());
}

#[sqlx::test(migrations = "./migrations")]
async fn email_responses_one_per_status_and_admin_managed(pool: Pool) {
    let (a, pa, b, _pb) = two_tenants(&pool).await;
    let agent = user(&pool, "agent@example.com").await;
    add_member(&pool, a.id, pa.id, agent.id, Role::Agent).await;

    assert!(
        email_responses::create(&pool, agent.id, pa.id, Status::Created, "", "x", true)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        email_responses::create(&pool, b.id, pa.id, Status::Created, "", "x", true)
            .await
            .unwrap()
            .is_none()
    );
    let created = email_responses::create(
        &pool,
        a.id,
        pa.id,
        Status::Created,
        "Got it",
        "We got it",
        true,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        (created.status, created.subject.as_str(), created.is_enabled),
        (Status::Created, "Got it", true)
    );
    let resolved = email_responses::create(&pool, a.id, pa.id, Status::Resolved, "", "Done", false)
        .await
        .unwrap()
        .unwrap();
    let dup = email_responses::create(&pool, a.id, pa.id, Status::Created, "", "again", true)
        .await
        .unwrap_err();
    assert!(sd_db::is_unique_violation(&dup));
    assert_eq!(
        constraint(&dup),
        Some("email_responses_project_id_status_key")
    );

    assert_eq!(
        email_responses::list(&pool, agent.id, pa.id, None)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        email_responses::list(&pool, agent.id, pa.id, Some(Status::Resolved))
            .await
            .unwrap()[0]
            .id,
        resolved.id
    );
    assert!(
        email_responses::list(&pool, b.id, pa.id, None)
            .await
            .unwrap()
            .is_empty()
    );

    // System side sees only enabled ones.
    assert!(
        email_responses::get_enabled(&pool, pa.id, Status::Created)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        email_responses::get_enabled(&pool, pa.id, Status::Resolved)
            .await
            .unwrap()
            .is_none()
    );

    assert!(
        email_responses::update(&pool, agent.id, resolved.id, None, None, Some(true))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        email_responses::update(&pool, b.id, resolved.id, None, None, Some(true))
            .await
            .unwrap()
            .is_none()
    );
    let up = email_responses::update(
        &pool,
        a.id,
        resolved.id,
        Some("Closed!"),
        Some("Fixed"),
        Some(true),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        (up.subject.as_str(), up.body.as_str(), up.is_enabled),
        ("Closed!", "Fixed", true)
    );
    assert!(
        email_responses::get_enabled(&pool, pa.id, Status::Resolved)
            .await
            .unwrap()
            .is_some()
    );
    let cleared = email_responses::update(&pool, a.id, resolved.id, Some(""), None, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cleared.subject, "");

    assert!(
        !email_responses::delete(&pool, agent.id, resolved.id)
            .await
            .unwrap()
    );
    assert!(
        !email_responses::delete(&pool, b.id, resolved.id)
            .await
            .unwrap()
    );
    assert!(
        email_responses::delete(&pool, a.id, resolved.id)
            .await
            .unwrap()
    );
    assert!(
        !email_responses::delete(&pool, a.id, resolved.id)
            .await
            .unwrap()
    );

    let bad = sqlx::query("INSERT INTO email_responses (id, project_id, status, body) VALUES ($1, $2, 'pending', 'x')")
        .bind(new_id()).bind(pa.id).execute(&pool).await;
    assert!(bad.is_err(), "only created/resolved are valid");
}

#[sqlx::test(migrations = "./migrations")]
async fn email_log_lifecycle_and_tenant_consistency(pool: Pool) {
    let (a, pa, b, pb) = two_tenants(&pool).await;
    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;
    let new = NewEmailLog {
        project_id: pa.id,
        ticket_id: t.id,
        message_id: None,
        to_email: "c@example.com".into(),
        subject: Some("Re: Printer".into()),
        sent_by_user_id: Some(a.id),
    };
    let log = email_logs::create(&pool, &new).await.unwrap();
    assert_eq!(
        (log.status, log.smtp_message_id.as_deref()),
        (EmailLogStatus::Pending, None)
    );

    assert!(
        email_logs::mark_failed(&pool, log.id, "550 mailbox full")
            .await
            .unwrap()
    );
    let logs = email_logs::list_for_ticket(&pool, a.id, t.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (logs[0].status, logs[0].error.as_deref()),
        (EmailLogStatus::Failed, Some("550 mailbox full"))
    );
    assert!(
        email_logs::mark_sent(&pool, log.id, "<id@smtp>")
            .await
            .unwrap()
    );
    let logs = email_logs::list_for_ticket(&pool, a.id, t.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            logs[0].status,
            logs[0].smtp_message_id.as_deref(),
            logs[0].error.as_deref()
        ),
        (EmailLogStatus::Sent, Some("<id@smtp>"), None)
    );
    assert!(!email_logs::mark_sent(&pool, new_id(), "x").await.unwrap());
    assert!(!email_logs::mark_failed(&pool, new_id(), "x").await.unwrap());

    let second = email_logs::create(&pool, &new).await.unwrap();
    let logs = email_logs::list_for_ticket(&pool, a.id, t.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        logs.iter().map(|l| l.id).collect::<Vec<_>>(),
        vec![second.id, log.id],
        "newest first"
    );
    assert!(
        email_logs::list_for_ticket(&pool, b.id, t.id)
            .await
            .unwrap()
            .is_none()
    );

    // A log must not mix tenant B's project with tenant A's ticket.
    let mixed = NewEmailLog {
        project_id: pb.id,
        ..new.clone()
    };
    assert!(sd_db::is_foreign_key_violation(
        &email_logs::create(&pool, &mixed).await.unwrap_err()
    ));

    // The message link is dropped, the log kept, when a message is deleted.
    let m: sd_db::Id = sqlx::query_scalar("SELECT id FROM messages WHERE ticket_id = $1")
        .bind(t.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let linked = email_logs::create(
        &pool,
        &NewEmailLog {
            message_id: Some(m),
            ..new
        },
    )
    .await
    .unwrap();
    assert_eq!(linked.message_id, Some(m));
    sqlx::query("DELETE FROM messages WHERE id = $1")
        .bind(m)
        .execute(&pool)
        .await
        .unwrap();
    let logs = email_logs::list_for_ticket(&pool, a.id, t.id)
        .await
        .unwrap()
        .unwrap();
    assert!(logs.iter().all(|l| l.message_id.is_none()) && logs.len() == 3);
    sd_db::tickets::delete(&pool, a.id, t.id).await.unwrap();
    assert_eq!(count(&pool, "email_logs").await, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn unread_list_mark_and_clear(pool: Pool) {
    let (a, pa, b, pb) = two_tenants(&pool).await;
    let second = project(&pool, a.id, "tenant-a2").await;
    let t1 = ticket(&pool, a.id, pa.id, "1@example.com").await;
    let t2 = ticket(&pool, a.id, second.id, "2@example.com").await;
    let tb = ticket(&pool, b.id, pb.id, "3@example.com").await;

    let all = unread::list(&pool, a.id, None).await.unwrap();
    assert_eq!(
        all.iter().map(|u| u.ticket_id).collect::<Vec<_>>(),
        vec![t1.id, t2.id],
        "oldest first, own tenants only"
    );
    let one = unread::list(&pool, a.id, Some(second.id)).await.unwrap();
    assert_eq!((one.len(), one[0].project_id), (1, second.id));
    assert!(
        unread::list(&pool, a.id, Some(pb.id))
            .await
            .unwrap()
            .is_empty()
    );

    // Clearing is scoped: tenant B cannot clear A's marker.
    assert_eq!(unread::clear(&pool, b.id, t1.id).await.unwrap(), 0);
    assert_eq!(unread::clear(&pool, a.id, t1.id).await.unwrap(), 1);
    assert_eq!(unread::clear(&pool, a.id, t1.id).await.unwrap(), 0);

    // mark is idempotent and checks the project/ticket pair.
    assert!(unread::mark(&pool, pa.id, t1.id).await.unwrap());
    assert!(
        !unread::mark(&pool, pa.id, t1.id).await.unwrap(),
        "already unread"
    );
    assert!(
        !unread::mark(&pool, pb.id, t1.id).await.unwrap(),
        "wrong project"
    );
    assert!(!unread::mark(&pool, pa.id, tb.id).await.unwrap());
    assert_eq!(count(&pool, "unread_tickets").await, 3);
    let dup =
        sqlx::query("INSERT INTO unread_tickets (id, project_id, ticket_id) VALUES ($1, $2, $3)")
            .bind(new_id())
            .bind(pa.id)
            .bind(t1.id)
            .execute(&pool)
            .await;
    assert!(dup.is_err(), "one row per ticket");
}

#[sqlx::test(migrations = "./migrations")]
async fn viewers_heartbeat_expire_and_scope(pool: Pool) {
    let (a, pa, b, _pb) = two_tenants(&pool).await;
    let agent = user(&pool, "agent@example.com").await;
    add_member(&pool, a.id, pa.id, agent.id, Role::Agent).await;
    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;

    assert!(viewers::touch(&pool, a.id, t.id).await.unwrap());
    assert!(
        viewers::touch(&pool, a.id, t.id).await.unwrap(),
        "heartbeat is an upsert"
    );
    assert!(viewers::touch(&pool, agent.id, t.id).await.unwrap());
    assert!(
        !viewers::touch(&pool, b.id, t.id).await.unwrap(),
        "outsiders cannot register"
    );
    assert_eq!(count(&pool, "active_ticket_viewers").await, 2);

    let live = viewers::list(&pool, agent.id, t.id, 120)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        live.iter().map(|v| v.user_id).collect::<Vec<_>>(),
        vec![a.id, agent.id]
    );
    assert_eq!(live[0].name, "User a@example.com");
    assert!(
        viewers::list(&pool, b.id, t.id, 120)
            .await
            .unwrap()
            .is_none()
    );

    sqlx::query("UPDATE active_ticket_viewers SET last_seen_at = now() - interval '3 minutes' WHERE user_id = $1")
        .bind(a.id).execute(&pool).await.unwrap();
    let live = viewers::list(&pool, agent.id, t.id, 120)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        live.iter().map(|v| v.user_id).collect::<Vec<_>>(),
        vec![agent.id],
        "stale heartbeats are hidden"
    );
    assert_eq!(viewers::purge_stale(&pool, 120).await.unwrap(), 1);
    assert_eq!(count(&pool, "active_ticket_viewers").await, 1);

    assert!(viewers::remove(&pool, agent.id, t.id).await.unwrap());
    assert!(!viewers::remove(&pool, agent.id, t.id).await.unwrap());
}
