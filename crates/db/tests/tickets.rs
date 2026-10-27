mod common;

use common::*;
use sd_db::tickets::{Access, TicketFilter, TicketPatch, TicketStats};
use sd_db::{
    Page, Pool, Priority, Role, TicketSource, TicketStatus, clients, is_foreign_key_violation,
    members, new_id, projects, tickets,
};
use std::collections::BTreeSet;

#[sqlx::test(migrations = "./migrations")]
async fn create_makes_client_message_unread_and_number_one(pool: Pool) {
    let (a, pa, _b, _pb) = two_tenants(&pool).await;
    let t = ticket(&pool, a.id, pa.id, "Cust@Example.com").await;
    assert_eq!(
        (t.ticket_number, t.status, t.priority, t.source),
        (
            1,
            TicketStatus::Pending,
            Priority::Normal,
            TicketSource::Web
        )
    );
    assert_eq!(
        (
            t.client_email.as_str(),
            t.client_name.as_deref(),
            t.project_name.as_str()
        ),
        ("Cust@Example.com", Some("Client"), "Project tenant-a")
    );
    assert_eq!(count(&pool, "messages").await, 1);
    let m: (String, String, bool) =
        sqlx::query_as("SELECT sender_type, body, is_internal FROM messages")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        m,
        ("client".to_string(), "It is burning".to_string(), false)
    );
    assert_eq!(count(&pool, "unread_tickets").await, 1);

    // Same client (any case) is reused, numbers advance.
    let t2 = ticket(&pool, a.id, pa.id, "cust@example.com").await;
    assert_eq!((t2.ticket_number, t2.client_id), (2, t.client_id));
    assert_eq!(count(&pool, "clients").await, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn ticket_numbers_are_per_project(pool: Pool) {
    let (a, pa, b, pb) = two_tenants(&pool).await;
    for _ in 0..3 {
        ticket(&pool, a.id, pa.id, "c@example.com").await;
    }
    let first_b = ticket(&pool, b.id, pb.id, "c@example.com").await;
    assert_eq!(first_b.ticket_number, 1);
    let next_a = ticket(&pool, a.id, pa.id, "c@example.com").await;
    assert_eq!(next_a.ticket_number, 4);
    // Same email in two tenants is two different clients.
    assert_eq!(count(&pool, "clients").await, 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn fifty_parallel_creates_get_numbers_1_to_50(pool: Pool) {
    let (a, pa, _b, _pb) = two_tenants(&pool).await;
    let mut set = tokio::task::JoinSet::new();
    for i in 0..50 {
        let pool = pool.clone();
        set.spawn(async move {
            // Mix of repeat and distinct clients to also race the client upsert.
            let email = format!("client{}@example.com", i % 7);
            tickets::create(&pool, &Access::Member(a.id), &new_ticket(pa.id, &email))
                .await
                .unwrap()
                .unwrap()
                .ticket_number
        });
    }
    let mut numbers = BTreeSet::new();
    while let Some(n) = set.join_next().await {
        assert!(numbers.insert(n.unwrap()), "duplicate ticket number");
    }
    assert_eq!(numbers, (1..=50).collect::<BTreeSet<i64>>());
    assert_eq!(count(&pool, "clients").await, 7);
    let next: i64 = sqlx::query_scalar("SELECT next_ticket_number FROM projects WHERE id = $1")
        .bind(pa.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(next, 51);
}

#[sqlx::test(migrations = "./migrations")]
async fn duplicate_number_in_a_project_is_rejected_by_the_database(pool: Pool) {
    let (a, pa, _b, _pb) = two_tenants(&pool).await;
    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;
    let dup = sqlx::query("INSERT INTO tickets (id, project_id, ticket_number, client_id, subject) VALUES ($1, $2, 1, $3, 's')")
        .bind(new_id()).bind(pa.id).bind(t.client_id).execute(&pool).await.unwrap_err();
    assert!(sd_db::is_unique_violation(&dup));
}

#[sqlx::test(migrations = "./migrations")]
async fn create_checks_access(pool: Pool) {
    let (a, pa, b, _pb) = two_tenants(&pool).await;
    let nt = new_ticket(pa.id, "c@example.com");
    assert!(
        tickets::create(&pool, &Access::Member(b.id), &nt)
            .await
            .unwrap()
            .is_none(),
        "non-member"
    );
    assert!(
        tickets::create(&pool, &Access::ApiKeyHash("wrong".into()), &nt)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        tickets::create(&pool, &Access::ApiKeyHash("hash-tenant-b".into()), &nt)
            .await
            .unwrap()
            .is_none(),
        "another tenant's key"
    );
    assert_eq!(count(&pool, "tickets").await, 0);
    assert_eq!(
        count(&pool, "clients").await,
        0,
        "a refused create leaves nothing behind"
    );

    let via_key = tickets::create(&pool, &Access::ApiKeyHash("hash-tenant-a".into()), &nt)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(via_key.created_by_user_id, None);
    let mut admin = new_ticket(pa.id, "d@example.com");
    admin.source = TicketSource::Admin;
    admin.created_by = Some(a.id);
    let by_admin = tickets::create(&pool, &Access::Member(a.id), &admin)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (by_admin.created_by_user_id, by_admin.source),
        (Some(a.id), TicketSource::Admin)
    );
    let sys = tickets::create(&pool, &Access::System, &new_ticket(pa.id, "e@example.com"))
        .await
        .unwrap();
    assert!(sys.is_some());
    assert!(
        tickets::create(
            &pool,
            &Access::System,
            &new_ticket(new_id(), "e@example.com")
        )
        .await
        .unwrap()
        .is_none()
    );

    projects::update(
        &pool,
        a.id,
        pa.id,
        &projects::ProjectPatch {
            status: Some(sd_db::Status::Suspended),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        tickets::create(&pool, &Access::ApiKeyHash("hash-tenant-a".into()), &nt)
            .await
            .unwrap()
            .is_none()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn tenant_isolation_on_reads_and_writes(pool: Pool) {
    let (a, pa, b, pb) = two_tenants(&pool).await;
    let ta = ticket(&pool, a.id, pa.id, "c@example.com").await;
    let tb = ticket(&pool, b.id, pb.id, "c@example.com").await;

    assert!(tickets::get(&pool, b.id, ta.id).await.unwrap().is_none());
    assert!(
        tickets::update(
            &pool,
            b.id,
            ta.id,
            &TicketPatch {
                subject: Some("pwn".into()),
                ..Default::default()
            }
        )
        .await
        .unwrap()
        .is_none()
    );
    assert!(!tickets::delete(&pool, b.id, ta.id).await.unwrap());
    assert_eq!(
        tickets::get(&pool, a.id, ta.id)
            .await
            .unwrap()
            .unwrap()
            .subject,
        "Printer on fire"
    );

    let (rows, total) = tickets::list(&pool, b.id, &TicketFilter::default(), Page::new(50, 0))
        .await
        .unwrap();
    assert_eq!((rows.len(), total, rows[0].id), (1, 1, tb.id));
    let filter = TicketFilter {
        project_id: Some(pa.id),
        ..Default::default()
    };
    let (rows, total) = tickets::list(&pool, b.id, &filter, Page::new(50, 0))
        .await
        .unwrap();
    assert_eq!(
        (rows.len(), total),
        (0, 0),
        "foreign project filter yields nothing"
    );
    let (found, _) = tickets::search(&pool, b.id, "printer", None, Page::new(50, 0))
        .await
        .unwrap();
    assert!(found.iter().all(|t| t.project_id == pb.id));
    assert_eq!(
        tickets::stats(&pool, b.id, Some(pa.id)).await.unwrap(),
        TicketStats::default()
    );
    let (global, per) = tickets::counts(&pool, b.id).await.unwrap();
    assert_eq!((global.all, per.len()), (1, 1));
}

#[sqlx::test(migrations = "./migrations")]
async fn cross_tenant_rows_are_impossible_even_with_raw_sql(pool: Pool) {
    let (a, pa, b, pb) = two_tenants(&pool).await;
    let ta = ticket(&pool, a.id, pa.id, "c@example.com").await;
    let tb = ticket(&pool, b.id, pb.id, "c@example.com").await;
    // A ticket in tenant B pointing at tenant A's client.
    let e = sqlx::query("INSERT INTO tickets (id, project_id, ticket_number, client_id, subject) VALUES ($1, $2, 99, $3, 's')")
        .bind(new_id()).bind(pb.id).bind(ta.client_id).execute(&pool).await.unwrap_err();
    assert!(is_foreign_key_violation(&e));
    // A message in tenant B's name on tenant A's ticket.
    let e = sqlx::query("INSERT INTO messages (id, project_id, ticket_id, sender_type, body) VALUES ($1, $2, $3, 'client', 'x')")
        .bind(new_id()).bind(pb.id).bind(ta.id).execute(&pool).await.unwrap_err();
    assert!(is_foreign_key_violation(&e));
    // Assigning a ticket to a user from another tenant.
    let e = tickets::update(
        &pool,
        b.id,
        tb.id,
        &TicketPatch {
            assignee: Some(Some(a.id)),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(is_foreign_key_violation(&e));
}

#[sqlx::test(migrations = "./migrations")]
async fn can_view_all_tickets_false_limits_to_assigned_or_created(pool: Pool) {
    let (a, pa, _b, _pb) = two_tenants(&pool).await;
    let limited = user(&pool, "limited@example.com").await;
    add_member(&pool, a.id, pa.id, limited.id, Role::Agent).await;
    members::update(
        &pool,
        a.id,
        pa.id,
        limited.id,
        &members::MemberPatch {
            can_view_all_tickets: Some(false),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();

    let mine = ticket(&pool, a.id, pa.id, "1@example.com").await;
    let created = tickets::create(&pool, &Access::Member(limited.id), &{
        let mut n = new_ticket(pa.id, "2@example.com");
        n.created_by = Some(limited.id);
        n
    })
    .await
    .unwrap()
    .unwrap();
    let other = ticket(&pool, a.id, pa.id, "3@example.com").await;
    tickets::update(
        &pool,
        a.id,
        mine.id,
        &TicketPatch {
            assignee: Some(Some(limited.id)),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();

    let (rows, total) = tickets::list(
        &pool,
        limited.id,
        &TicketFilter::default(),
        Page::new(50, 0),
    )
    .await
    .unwrap();
    let ids: BTreeSet<_> = rows.iter().map(|t| t.id).collect();
    assert_eq!(total, 2);
    assert_eq!(ids, BTreeSet::from([mine.id, created.id]));
    assert!(
        tickets::get(&pool, limited.id, other.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        tickets::stats(&pool, limited.id, None).await.unwrap().all,
        2
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn list_filters_order_and_pagination(pool: Pool) {
    let (a, pa, _b, _pb) = two_tenants(&pool).await;
    let helper = user(&pool, "h@example.com").await;
    add_member(&pool, a.id, pa.id, helper.id, Role::Agent).await;
    let t1 = ticket(&pool, a.id, pa.id, "1@example.com").await;
    let t2 = ticket(&pool, a.id, pa.id, "2@example.com").await;
    let t3 = ticket(&pool, a.id, pa.id, "3@example.com").await;
    tickets::update(
        &pool,
        a.id,
        t1.id,
        &TicketPatch {
            status: Some(TicketStatus::InProgress),
            priority: Some(Priority::Urgent),
            assignee: Some(Some(helper.id)),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    sd_db::unread::clear(&pool, a.id, t3.id).await.unwrap();

    let ids = |rows: &[sd_db::tickets::Ticket]| rows.iter().map(|t| t.id).collect::<Vec<_>>();
    let page = Page::new(50, 0);
    let (all, total) = tickets::list(&pool, a.id, &TicketFilter::default(), page)
        .await
        .unwrap();
    assert_eq!(total, 3);
    assert_eq!(
        ids(&all),
        vec![t1.id, t3.id, t2.id],
        "most recently updated first"
    );

    // Unread tickets float up; t3 is read.
    let unread_first = TicketFilter {
        unread_first: true,
        ..Default::default()
    };
    let (rows, _) = tickets::list(&pool, a.id, &unread_first, page)
        .await
        .unwrap();
    assert_eq!(ids(&rows), vec![t1.id, t2.id, t3.id]);

    let by_status = TicketFilter {
        status: Some(TicketStatus::InProgress),
        ..Default::default()
    };
    assert_eq!(
        ids(&tickets::list(&pool, a.id, &by_status, page)
            .await
            .unwrap()
            .0),
        vec![t1.id]
    );
    let by_prio = TicketFilter {
        priority: Some(Priority::Urgent),
        ..Default::default()
    };
    assert_eq!(
        ids(&tickets::list(&pool, a.id, &by_prio, page).await.unwrap().0),
        vec![t1.id]
    );
    let by_assignee = TicketFilter {
        assigned_to: Some(helper.id),
        ..Default::default()
    };
    assert_eq!(
        ids(&tickets::list(&pool, a.id, &by_assignee, page)
            .await
            .unwrap()
            .0),
        vec![t1.id]
    );
    let by_project = TicketFilter {
        project_id: Some(pa.id),
        ..Default::default()
    };
    assert_eq!(
        tickets::list(&pool, a.id, &by_project, page)
            .await
            .unwrap()
            .1,
        3
    );

    let (p1, total) = tickets::list(&pool, a.id, &TicketFilter::default(), Page::new(2, 0))
        .await
        .unwrap();
    let (p2, _) = tickets::list(&pool, a.id, &TicketFilter::default(), Page::new(2, 2))
        .await
        .unwrap();
    assert_eq!((p1.len(), p2.len(), total), (2, 1, 3));
    // Limits are clamped, never unbounded or zero.
    assert_eq!(Page::new(10_000, -5).limit, sd_db::MAX_LIMIT);
    assert_eq!((Page::new(0, -5).limit, Page::new(0, -5).offset), (1, 0));
}

#[sqlx::test(migrations = "./migrations")]
async fn stats_and_counts(pool: Pool) {
    let (a, pa, _b, pb) = two_tenants(&pool).await;
    let second = project(&pool, a.id, "tenant-a2").await;
    let t1 = ticket(&pool, a.id, pa.id, "1@example.com").await;
    ticket(&pool, a.id, pa.id, "2@example.com").await;
    let t3 = ticket(&pool, a.id, second.id, "3@example.com").await;
    let set = |s| TicketPatch {
        status: Some(s),
        ..Default::default()
    };
    tickets::update(&pool, a.id, t1.id, &set(TicketStatus::Resolved))
        .await
        .unwrap()
        .unwrap();
    tickets::update(&pool, a.id, t3.id, &set(TicketStatus::OnHold))
        .await
        .unwrap()
        .unwrap();

    let s = tickets::stats(&pool, a.id, None).await.unwrap();
    assert_eq!(
        s,
        TicketStats {
            pending: 1,
            in_progress: 0,
            on_hold: 1,
            resolved: 1,
            closed: 0,
            all: 3
        }
    );
    let s = tickets::stats(&pool, a.id, Some(pa.id)).await.unwrap();
    assert_eq!((s.pending, s.resolved, s.all), (1, 1, 2));

    let (global, per) = tickets::counts(&pool, a.id).await.unwrap();
    assert_eq!(global.all, 3);
    let get = |p| {
        per.iter()
            .find(|(id, _)| *id == p)
            .map(|(_, s)| *s)
            .unwrap()
    };
    assert_eq!(get(pa.id).all, 2);
    assert_eq!(get(second.id).on_hold, 1);
    assert!(per.iter().all(|(id, _)| *id != pb.id));
}

#[sqlx::test(migrations = "./migrations")]
async fn search_covers_subject_description_and_messages(pool: Pool) {
    let (a, pa, _b, _pb) = two_tenants(&pool).await;
    let mut n = new_ticket(pa.id, "1@example.com");
    n.subject = "Invoice missing".into();
    n.description = Some("see attached".into());
    n.first_message = "see attached".into();
    let t1 = tickets::create(&pool, &Access::Member(a.id), &n)
        .await
        .unwrap()
        .unwrap();
    let mut n = new_ticket(pa.id, "2@example.com");
    n.subject = "Login broken".into();
    n.description = Some("Error 100% reproducible".into());
    n.first_message = "password reset loop".into();
    let t2 = tickets::create(&pool, &Access::Member(a.id), &n)
        .await
        .unwrap()
        .unwrap();

    let page = Page::new(50, 0);
    let find = |q: &'static str| {
        let pool = pool.clone();
        async move { tickets::search(&pool, a.id, q, None, page).await.unwrap() }
    };
    assert_eq!(find("INVOICE").await.0[0].id, t1.id);
    assert_eq!(find("100%").await.0[0].id, t2.id);
    assert_eq!(
        find("reset loop").await.0[0].id,
        t2.id,
        "matches message bodies"
    );
    assert_eq!(find("nothing-like-this").await.1, 0);
    assert_eq!(find("x").await.1, 0, "shorter than 2 chars matches nothing");
    assert_eq!(find("  ").await.1, 0);
    assert_eq!(find("%%").await.1, 0, "wildcards are literal");
    let scoped = tickets::search(&pool, a.id, "invoice", Some(new_id()), page)
        .await
        .unwrap();
    assert_eq!(scoped.1, 0);
    let (rows, total) = tickets::search(&pool, a.id, "in", None, Page::new(1, 0))
        .await
        .unwrap();
    assert_eq!(
        (rows.len(), total),
        (1, 2),
        "total counts all matches, the page only one"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn update_tracks_status_history_and_assignment(pool: Pool) {
    let (a, pa, _b, _pb) = two_tenants(&pool).await;
    let helper = user(&pool, "h@example.com").await;
    add_member(&pool, a.id, pa.id, helper.id, Role::Agent).await;
    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;
    assert!(t.closed_at.is_none());

    let up = tickets::update(
        &pool,
        a.id,
        t.id,
        &TicketPatch {
            status: Some(TicketStatus::Resolved),
            subject: Some("New subject".into()),
            description: Some("New description".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(up.previous_status, TicketStatus::Pending);
    assert_eq!(
        (up.ticket.status, up.ticket.subject.as_str()),
        (TicketStatus::Resolved, "New subject")
    );
    let closed_at = up.ticket.closed_at.expect("resolved stamps closed_at");

    // Resolved -> closed keeps the first timestamp.
    let up = tickets::update(
        &pool,
        a.id,
        t.id,
        &TicketPatch {
            status: Some(TicketStatus::Closed),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        (up.previous_status, up.ticket.closed_at),
        (TicketStatus::Resolved, Some(closed_at))
    );

    // Reopening clears it (the TS product never did).
    let up = tickets::update(
        &pool,
        a.id,
        t.id,
        &TicketPatch {
            status: Some(TicketStatus::InProgress),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert!(up.ticket.closed_at.is_none());

    // A patch without status leaves closed_at alone.
    let up = tickets::update(
        &pool,
        a.id,
        t.id,
        &TicketPatch {
            priority: Some(Priority::High),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(up.ticket.priority, Priority::High);

    let up = tickets::update(
        &pool,
        a.id,
        t.id,
        &TicketPatch {
            assignee: Some(Some(helper.id)),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        (
            up.ticket.assigned_to_user_id,
            up.ticket.assignee_name.as_deref()
        ),
        (Some(helper.id), Some("User h@example.com"))
    );
    let up = tickets::update(&pool, a.id, t.id, &TicketPatch::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        up.ticket.assigned_to_user_id,
        Some(helper.id),
        "no assignee field = unchanged"
    );
    let up = tickets::update(
        &pool,
        a.id,
        t.id,
        &TicketPatch {
            assignee: Some(None),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(up.ticket.assigned_to_user_id, None);

    assert!(
        tickets::update(&pool, a.id, new_id(), &TicketPatch::default())
            .await
            .unwrap()
            .is_none()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn assignee_must_be_a_member_of_the_project(pool: Pool) {
    let (a, pa, b, _pb) = two_tenants(&pool).await;
    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;
    let err = tickets::update(
        &pool,
        a.id,
        t.id,
        &TicketPatch {
            assignee: Some(Some(b.id)),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(is_foreign_key_violation(&err));
    assert_eq!(
        tickets::get(&pool, a.id, t.id)
            .await
            .unwrap()
            .unwrap()
            .assigned_to_user_id,
        None
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn check_constraints_on_ticket_fields(pool: Pool) {
    let (a, pa, _b, _pb) = two_tenants(&pool).await;
    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;
    for (col, val) in [
        ("status", "done"),
        ("priority", "meh"),
        ("source", "fax"),
        ("subject", ""),
    ] {
        let r = sqlx::query(&format!("UPDATE tickets SET {col} = $1 WHERE id = $2"))
            .bind(val)
            .bind(t.id)
            .execute(&pool)
            .await;
        assert!(r.is_err(), "{col}={val} must be rejected");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn find_by_number_is_project_scoped(pool: Pool) {
    let (a, pa, b, pb) = two_tenants(&pool).await;
    let ta = ticket(&pool, a.id, pa.id, "c@example.com").await;
    ticket(&pool, b.id, pb.id, "c@example.com").await;
    assert_eq!(
        tickets::find_by_number(&pool, pa.id, 1).await.unwrap(),
        Some(ta.id)
    );
    assert_eq!(
        tickets::find_by_number(&pool, pa.id, 2).await.unwrap(),
        None
    );
    assert_ne!(
        tickets::find_by_number(&pool, pb.id, 1).await.unwrap(),
        Some(ta.id)
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn delete_needs_admin_and_cascades(pool: Pool) {
    let (a, pa, _b, _pb) = two_tenants(&pool).await;
    let agent = user(&pool, "agent@example.com").await;
    add_member(&pool, a.id, pa.id, agent.id, Role::Agent).await;
    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;
    sd_db::viewers::touch(&pool, a.id, t.id).await.unwrap();

    assert!(!tickets::delete(&pool, agent.id, t.id).await.unwrap());
    assert!(tickets::delete(&pool, a.id, t.id).await.unwrap());
    assert!(!tickets::delete(&pool, a.id, t.id).await.unwrap());
    for table in [
        "tickets",
        "messages",
        "unread_tickets",
        "active_ticket_viewers",
    ] {
        assert_eq!(count(&pool, table).await, 0, "{table}");
    }
    assert_eq!(
        count(&pool, "clients").await,
        1,
        "clients outlive their tickets"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn client_with_tickets_survives_only_with_its_project(pool: Pool) {
    let (a, pa, _b, _pb) = two_tenants(&pool).await;
    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;
    let err = sqlx::query("DELETE FROM clients WHERE id = $1")
        .bind(t.client_id)
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(
        is_foreign_key_violation(&err),
        "a client with tickets cannot be deleted on its own"
    );
    assert!(
        clients::get(&pool, a.id, t.client_id)
            .await
            .unwrap()
            .is_some()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn auto_close_only_old_resolved_tickets_of_projects_that_want_it(pool: Pool) {
    let (a, pa, b, pb) = two_tenants(&pool).await;
    let old = ticket(&pool, a.id, pa.id, "1@example.com").await;
    let fresh = ticket(&pool, a.id, pa.id, "2@example.com").await;
    let pending = ticket(&pool, a.id, pa.id, "3@example.com").await;
    let never = ticket(&pool, b.id, pb.id, "4@example.com").await;
    let resolve = TicketPatch {
        status: Some(TicketStatus::Resolved),
        ..Default::default()
    };
    for id in [old.id, fresh.id] {
        tickets::update(&pool, a.id, id, &resolve)
            .await
            .unwrap()
            .unwrap();
    }
    tickets::update(&pool, b.id, never.id, &resolve)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE tickets SET updated_at = now() - interval '8 days' WHERE id = ANY($1)")
        .bind(vec![old.id, never.id, pending.id])
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE projects SET auto_close_days = 0 WHERE id = $1")
        .bind(pb.id)
        .execute(&pool)
        .await
        .unwrap();

    let closed = tickets::auto_close_resolved(&pool).await.unwrap();
    assert_eq!(closed, vec![(pa.id, old.id)]);
    let s = tickets::get(&pool, a.id, old.id).await.unwrap().unwrap();
    assert_eq!(s.status, TicketStatus::Closed);
    assert!(s.closed_at.is_some());
    assert_eq!(
        tickets::get(&pool, a.id, fresh.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        TicketStatus::Resolved
    );
    assert_eq!(
        tickets::get(&pool, a.id, pending.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        TicketStatus::Pending
    );
    assert_eq!(
        tickets::get(&pool, b.id, never.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        TicketStatus::Resolved
    );
    assert!(
        tickets::auto_close_resolved(&pool)
            .await
            .unwrap()
            .is_empty()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn client_upsert_and_listing(pool: Pool) {
    let (a, pa, b, pb) = two_tenants(&pool).await;
    let c1 = clients::upsert(&pool, pa.id, "Ann@Example.com", None)
        .await
        .unwrap();
    let c2 = clients::upsert(&pool, pa.id, "ann@example.com", Some("Ann"))
        .await
        .unwrap();
    assert_eq!(c1.id, c2.id);
    assert_eq!(
        c2.name.as_deref(),
        Some("Ann"),
        "a missing name is filled in"
    );
    let c3 = clients::upsert(&pool, pa.id, "ann@example.com", Some("Other"))
        .await
        .unwrap();
    assert_eq!(c3.name.as_deref(), Some("Ann"), "first name wins");
    let other_tenant = clients::upsert(&pool, pb.id, "ann@example.com", None)
        .await
        .unwrap();
    assert_ne!(other_tenant.id, c1.id);

    assert!(clients::get(&pool, a.id, c1.id).await.unwrap().is_some());
    assert!(clients::get(&pool, b.id, c1.id).await.unwrap().is_none());
    clients::upsert(&pool, pa.id, "bob@example.com", Some("Bob"))
        .await
        .unwrap();
    let page = Page::new(50, 0);
    assert_eq!(
        clients::list(&pool, a.id, pa.id, None, page)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        clients::list(&pool, a.id, pa.id, Some("BOB"), page)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        clients::list(&pool, a.id, pa.id, Some("%"), page)
            .await
            .unwrap()
            .len(),
        0
    );
    assert!(
        clients::list(&pool, b.id, pa.id, None, page)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        clients::list(&pool, a.id, pa.id, None, Page::new(1, 1))
            .await
            .unwrap()
            .len(),
        1
    );
}
