mod common;

use common::*;
use sd_db::attachments::{self, NewAttachment, Stored};
use sd_db::messages::{self, NewAgentMessage, NewInbound, Posted};
use sd_db::tickets::{self, TicketPatch};
use sd_db::{
    MessageType, Page, Pool, Role, SenderType, TicketStatus, is_unique_violation, new_id, unread,
};

fn reply(body: &str) -> NewAgentMessage {
    NewAgentMessage {
        body: body.into(),
        ..Default::default()
    }
}

fn inbound(project: sd_db::Id, ticket: sd_db::Id, message_id: Option<&str>) -> NewInbound {
    NewInbound {
        project_id: project,
        ticket_id: ticket,
        sender_email: "c@example.com".into(),
        sender_name: Some("Client".into()),
        body: "me again".into(),
        email_message_id: message_id.map(String::from),
        ..Default::default()
    }
}

fn file(key: &str, size: i64) -> NewAttachment {
    NewAttachment {
        filename: "a.png".into(),
        mime_type: "image/png".into(),
        size_bytes: size,
        storage_key: key.into(),
        message_id: None,
    }
}

async fn unwrap_posted(p: Posted) -> messages::Message {
    match p {
        Posted::Ok(m) => m,
        other => panic!("expected Ok, got {other:?}"),
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn agent_reply_sets_sender_bumps_ticket_and_first_response(pool: Pool) {
    let (a, pa, _b, _pb) = two_tenants(&pool).await;
    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;
    let m = unwrap_posted(
        messages::post_agent(
            &pool,
            a.id,
            t.id,
            &NewAgentMessage {
                body: "On it".into(),
                email_message_id: Some("<out-1@acme.com>".into()),
                email_in_reply_to: Some("<in-0@x>".into()),
                email_references: Some("<in-0@x>".into()),
                email_to: Some("c@example.com".into()),
                email_subject: Some("Re: Printer".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(
        (
            m.sender_type,
            m.message_type,
            m.is_internal,
            m.sender_user_id
        ),
        (SenderType::Agent, MessageType::Reply, false, Some(a.id))
    );
    assert_eq!(
        (m.sender_email.as_deref(), m.agent_name.as_deref()),
        (Some("a@example.com"), Some("User a@example.com"))
    );
    assert_eq!(m.email_message_id.as_deref(), Some("<out-1@acme.com>"));
    let after = tickets::get(&pool, a.id, t.id).await.unwrap().unwrap();
    assert!(after.first_response_at.is_some());
    assert!(after.updated_at > t.updated_at);
}

#[sqlx::test(migrations = "./migrations")]
async fn internal_notes_do_not_count_as_a_response(pool: Pool) {
    let (a, pa, _b, _pb) = two_tenants(&pool).await;
    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;
    let note = unwrap_posted(
        messages::post_agent(
            &pool,
            a.id,
            t.id,
            &NewAgentMessage {
                body: "private".into(),
                is_internal: true,
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(
        (note.message_type, note.is_internal),
        (MessageType::Note, true)
    );
    assert!(
        tickets::get(&pool, a.id, t.id)
            .await
            .unwrap()
            .unwrap()
            .first_response_at
            .is_none()
    );

    // The first public reply stamps it; later replies do not move it.
    unwrap_posted(
        messages::post_agent(&pool, a.id, t.id, &reply("hello"))
            .await
            .unwrap(),
    )
    .await;
    let first = tickets::get(&pool, a.id, t.id)
        .await
        .unwrap()
        .unwrap()
        .first_response_at
        .unwrap();
    unwrap_posted(
        messages::post_agent(&pool, a.id, t.id, &reply("again"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        tickets::get(&pool, a.id, t.id)
            .await
            .unwrap()
            .unwrap()
            .first_response_at,
        Some(first)
    );

    // Clients can never be internal (CHECK).
    let bad = sqlx::query(
        "UPDATE messages SET is_internal = true WHERE ticket_id = $1 AND sender_type = 'client'",
    )
    .bind(t.id)
    .execute(&pool)
    .await;
    assert!(bad.is_err());
}

#[sqlx::test(migrations = "./migrations")]
async fn posting_is_refused_for_other_tenants(pool: Pool) {
    let (a, pa, b, _pb) = two_tenants(&pool).await;
    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;
    assert!(matches!(
        messages::post_agent(&pool, b.id, t.id, &reply("x"))
            .await
            .unwrap(),
        Posted::NoAccess
    ));
    assert!(matches!(
        messages::post_agent(&pool, a.id, new_id(), &reply("x"))
            .await
            .unwrap(),
        Posted::NoAccess
    ));
    assert_eq!(count(&pool, "messages").await, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn list_is_newest_first_paginated_and_scoped(pool: Pool) {
    let (a, pa, b, _pb) = two_tenants(&pool).await;
    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;
    for i in 1..=4 {
        unwrap_posted(
            messages::post_agent(&pool, a.id, t.id, &reply(&format!("m{i}")))
                .await
                .unwrap(),
        )
        .await;
    }
    let (page, total) = messages::list(&pool, a.id, t.id, Page::new(2, 0))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            total,
            page.iter().map(|m| m.body.as_str()).collect::<Vec<_>>()
        ),
        (5, vec!["m4", "m3"])
    );
    let (page, _) = messages::list(&pool, a.id, t.id, Page::new(2, 4))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        page[0].body, "It is burning",
        "the client's first message is last"
    );
    assert_eq!(page[0].sender_type, SenderType::Client);
    assert!(
        messages::list(&pool, b.id, t.id, Page::new(50, 0))
            .await
            .unwrap()
            .is_none()
    );

    let one = messages::get(&pool, a.id, page[0].id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(one.ticket_id, t.id);
    assert!(
        messages::get(&pool, b.id, page[0].id)
            .await
            .unwrap()
            .is_none()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn attachments_link_to_the_message_that_claims_them(pool: Pool) {
    let (a, pa, _b, _pb) = two_tenants(&pool).await;
    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;
    let Stored::Ok(f1) = attachments::create(&pool, a.id, t.id, &file("k1", 10))
        .await
        .unwrap()
    else {
        panic!()
    };
    let Stored::Ok(f2) = attachments::create(&pool, a.id, t.id, &file("k2", 20))
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!((f1.message_id, f1.uploaded_by_user_id), (None, Some(a.id)));

    let m = unwrap_posted(
        messages::post_agent(
            &pool,
            a.id,
            t.id,
            &NewAgentMessage {
                body: "see files".into(),
                attachment_ids: vec![f1.id, f2.id, f1.id],
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(m.attachment_ids, vec![f1.id, f2.id]);
    let listed = attachments::get(&pool, a.id, f1.id).await.unwrap().unwrap();
    assert_eq!(listed.message_id, Some(m.id));
}

#[sqlx::test(migrations = "./migrations")]
async fn bad_attachment_ids_roll_the_whole_post_back(pool: Pool) {
    let (a, pa, _b, _pb) = two_tenants(&pool).await;
    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;
    let other_ticket = ticket(&pool, a.id, pa.id, "d@example.com").await;
    let Stored::Ok(mine) = attachments::create(&pool, a.id, t.id, &file("k1", 1))
        .await
        .unwrap()
    else {
        panic!()
    };
    let Stored::Ok(elsewhere) = attachments::create(&pool, a.id, other_ticket.id, &file("k2", 1))
        .await
        .unwrap()
    else {
        panic!()
    };
    let before = count(&pool, "messages").await;

    for ids in [vec![mine.id, elsewhere.id], vec![new_id()]] {
        let r = messages::post_agent(
            &pool,
            a.id,
            t.id,
            &NewAgentMessage {
                body: "x".into(),
                attachment_ids: ids,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(matches!(r, Posted::BadAttachments));
    }
    assert_eq!(count(&pool, "messages").await, before);
    assert!(
        attachments::get(&pool, a.id, mine.id)
            .await
            .unwrap()
            .unwrap()
            .message_id
            .is_none()
    );

    // Already linked files cannot be claimed twice.
    let m = unwrap_posted(
        messages::post_agent(
            &pool,
            a.id,
            t.id,
            &NewAgentMessage {
                body: "first".into(),
                attachment_ids: vec![mine.id],
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(m.attachment_ids, vec![mine.id]);
    let again = messages::post_agent(
        &pool,
        a.id,
        t.id,
        &NewAgentMessage {
            body: "second".into(),
            attachment_ids: vec![mine.id],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(matches!(again, Posted::BadAttachments));
}

#[sqlx::test(migrations = "./migrations")]
async fn inbound_appends_reopens_marks_unread_and_dedups(pool: Pool) {
    let (a, pa, _b, _pb) = two_tenants(&pool).await;
    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;
    tickets::update(
        &pool,
        a.id,
        t.id,
        &TicketPatch {
            status: Some(TicketStatus::Resolved),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    unread::clear(&pool, a.id, t.id).await.unwrap();
    assert_eq!(count(&pool, "unread_tickets").await, 0);

    let mut n = inbound(pa.id, t.id, Some("<c1@mail.example>"));
    n.html_body = Some("<p>me again</p>".into());
    n.email_in_reply_to = Some("<out-1@acme.com>".into());
    let m = messages::post_inbound(&pool, &n)
        .await
        .unwrap()
        .expect("stored");
    assert_eq!(
        (
            m.sender_type,
            m.html_body.as_deref(),
            m.email_in_reply_to.as_deref()
        ),
        (
            SenderType::Client,
            Some("<p>me again</p>"),
            Some("<out-1@acme.com>")
        )
    );
    let after = tickets::get(&pool, a.id, t.id).await.unwrap().unwrap();
    assert_eq!(after.status, TicketStatus::InProgress);
    assert!(after.closed_at.is_none());
    assert_eq!(count(&pool, "unread_tickets").await, 1);

    // Replayed Message-ID is a no-op.
    assert!(messages::post_inbound(&pool, &n).await.unwrap().is_none());
    assert_eq!(count(&pool, "messages").await, 2);
    // Messages without an id are never deduplicated.
    let plain = inbound(pa.id, t.id, None);
    assert!(
        messages::post_inbound(&pool, &plain)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        messages::post_inbound(&pool, &plain)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        count(&pool, "unread_tickets").await,
        1,
        "still a single unread row"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn inbound_cannot_cross_tenants_and_ids_are_unique_per_tenant(pool: Pool) {
    let (a, pa, b, pb) = two_tenants(&pool).await;
    let ta = ticket(&pool, a.id, pa.id, "c@example.com").await;
    let tb = ticket(&pool, b.id, pb.id, "c@example.com").await;
    // Tenant B cannot post into A's ticket by claiming it.
    assert!(
        messages::post_inbound(&pool, &inbound(pb.id, ta.id, Some("<x@y>")))
            .await
            .unwrap()
            .is_none()
    );
    // The same Message-ID in two tenants is fine (a mail cc'd to both).
    assert!(
        messages::post_inbound(&pool, &inbound(pa.id, ta.id, Some("<same@y>")))
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        messages::post_inbound(&pool, &inbound(pb.id, tb.id, Some("<same@y>")))
            .await
            .unwrap()
            .is_some()
    );
    // But raw duplicates inside one tenant hit the unique index.
    let e = sqlx::query("INSERT INTO messages (id, project_id, ticket_id, sender_type, body, email_message_id) VALUES ($1, $2, $3, 'client', 'b', '<same@y>')")
        .bind(new_id()).bind(pa.id).bind(ta.id).execute(&pool).await.unwrap_err();
    assert!(is_unique_violation(&e));
}

#[sqlx::test(migrations = "./migrations")]
async fn threading_lookup_by_message_ids(pool: Pool) {
    let (a, pa, b, pb) = two_tenants(&pool).await;
    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;
    let tb = ticket(&pool, b.id, pb.id, "c@example.com").await;
    unwrap_posted(
        messages::post_agent(
            &pool,
            a.id,
            t.id,
            &NewAgentMessage {
                body: "r".into(),
                email_message_id: Some("<out-1@acme.com>".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    )
    .await;
    messages::post_inbound(&pool, &inbound(pb.id, tb.id, Some("<other@b>")))
        .await
        .unwrap()
        .unwrap();

    let refs = vec!["<unknown@x>".to_string(), "<out-1@acme.com>".to_string()];
    assert_eq!(
        messages::find_ticket_by_message_ids(&pool, pa.id, &refs)
            .await
            .unwrap(),
        Some(t.id)
    );
    assert_eq!(
        messages::find_ticket_by_message_ids(&pool, pb.id, &refs)
            .await
            .unwrap(),
        None,
        "other tenants never match"
    );
    assert_eq!(
        messages::find_ticket_by_message_ids(&pool, pa.id, &[])
            .await
            .unwrap(),
        None
    );
    assert!(
        messages::message_id_exists(&pool, pa.id, "<out-1@acme.com>")
            .await
            .unwrap()
    );
    assert!(
        !messages::message_id_exists(&pool, pb.id, "<out-1@acme.com>")
            .await
            .unwrap()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn attachment_quota_scoping_and_cleanup_keys(pool: Pool) {
    let (a, pa, b, _pb) = two_tenants(&pool).await;
    let agent = user(&pool, "agent@example.com").await;
    add_member(&pool, a.id, pa.id, agent.id, Role::Agent).await;
    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;
    sqlx::query("UPDATE projects SET storage_quota_mb = 1 WHERE id = $1")
        .bind(pa.id)
        .execute(&pool)
        .await
        .unwrap();

    let Stored::Ok(f) = attachments::create(&pool, a.id, t.id, &file("k1", 600_000))
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(matches!(
        attachments::create(&pool, a.id, t.id, &file("k2", 600_000))
            .await
            .unwrap(),
        Stored::QuotaExceeded
    ));
    assert_eq!(
        attachments::usage_bytes(&pool, pa.id).await.unwrap(),
        600_000
    );
    assert!(
        matches!(
            attachments::create(&pool, a.id, t.id, &file("k3", 448_576))
                .await
                .unwrap(),
            Stored::Ok(_)
        ),
        "exactly the quota fits"
    );
    assert!(matches!(
        attachments::create(&pool, a.id, t.id, &file("k4", 1))
            .await
            .unwrap(),
        Stored::QuotaExceeded
    ));

    assert!(matches!(
        attachments::create(&pool, b.id, t.id, &file("k5", 1))
            .await
            .unwrap(),
        Stored::NoAccess
    ));
    assert!(matches!(
        attachments::create(&pool, a.id, new_id(), &file("k5", 1))
            .await
            .unwrap(),
        Stored::NoAccess
    ));

    assert!(attachments::get(&pool, a.id, f.id).await.unwrap().is_some());
    assert!(attachments::get(&pool, b.id, f.id).await.unwrap().is_none());
    assert_eq!(
        attachments::list_for_ticket(&pool, a.id, t.id)
            .await
            .unwrap()
            .unwrap()
            .len(),
        2
    );
    assert!(
        attachments::list_for_ticket(&pool, b.id, t.id)
            .await
            .unwrap()
            .is_none()
    );

    assert_eq!(
        attachments::storage_keys_for_ticket(&pool, a.id, t.id)
            .await
            .unwrap(),
        vec!["k1", "k3"]
    );
    assert!(
        attachments::storage_keys_for_ticket(&pool, agent.id, t.id)
            .await
            .unwrap()
            .is_empty(),
        "admin only"
    );
    assert!(
        attachments::storage_keys_for_ticket(&pool, b.id, t.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        attachments::storage_keys_for_project(&pool, a.id, pa.id)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(
        attachments::storage_keys_for_project(&pool, agent.id, pa.id)
            .await
            .unwrap()
            .is_empty(),
        "owner only"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn storage_keys_are_unique_and_rows_cascade(pool: Pool) {
    let (a, pa, _b, _pb) = two_tenants(&pool).await;
    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;
    attachments::create(&pool, a.id, t.id, &file("same", 1))
        .await
        .unwrap();
    let e = attachments::create(&pool, a.id, t.id, &file("same", 1))
        .await
        .unwrap_err();
    assert!(is_unique_violation(&e));

    // Deleting the message detaches the file, deleting the ticket removes it.
    let Stored::Ok(f) = attachments::create(&pool, a.id, t.id, &file("other", 1))
        .await
        .unwrap()
    else {
        panic!()
    };
    let m = unwrap_posted(
        messages::post_agent(
            &pool,
            a.id,
            t.id,
            &NewAgentMessage {
                body: "x".into(),
                attachment_ids: vec![f.id],
                ..Default::default()
            },
        )
        .await
        .unwrap(),
    )
    .await;
    sqlx::query("DELETE FROM messages WHERE id = $1")
        .bind(m.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        attachments::get(&pool, a.id, f.id)
            .await
            .unwrap()
            .unwrap()
            .message_id,
        None
    );
    assert!(tickets::delete(&pool, a.id, t.id).await.unwrap());
    assert_eq!(count(&pool, "attachments").await, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn system_attachments_must_match_project_and_message(pool: Pool) {
    let (a, pa, b, pb) = two_tenants(&pool).await;
    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;
    let tb = ticket(&pool, b.id, pb.id, "c@example.com").await;
    let m = messages::post_inbound(&pool, &inbound(pa.id, t.id, Some("<m@x>")))
        .await
        .unwrap()
        .unwrap();

    let mut f = file("in-1", 5);
    f.message_id = Some(m.id);
    let Stored::Ok(att) = attachments::create_for_project(&pool, pa.id, t.id, &f)
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        (att.message_id, att.uploaded_by_user_id),
        (Some(m.id), None)
    );
    let loaded = messages::get(&pool, a.id, m.id).await.unwrap().unwrap();
    assert_eq!(loaded.attachment_ids, vec![att.id]);

    let mut wrong_project = file("in-2", 5);
    wrong_project.message_id = None;
    assert!(matches!(
        attachments::create_for_project(&pool, pb.id, t.id, &wrong_project)
            .await
            .unwrap(),
        Stored::NoAccess
    ));
    // A message of another ticket cannot be claimed.
    let mut wrong_msg = file("in-3", 5);
    wrong_msg.message_id = Some(m.id);
    assert!(matches!(
        attachments::create_for_project(&pool, pb.id, tb.id, &wrong_msg)
            .await
            .unwrap(),
        Stored::NoAccess
    ));
}
