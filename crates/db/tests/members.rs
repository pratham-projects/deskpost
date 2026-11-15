mod common;

use common::*;
use dp_db::members::{self, MemberPatch};
use dp_db::{Page, Pool, Role, is_unique_violation, tickets};

#[sqlx::test(migrations = "./migrations")]
async fn add_list_and_role_lookup(pool: Pool) {
    let (a, pa, b, _pb) = two_tenants(&pool).await;
    let agent = user(&pool, "agent@example.com").await;

    assert_eq!(
        members::role_of(&pool, a.id, pa.id).await.unwrap(),
        Some(Role::Admin)
    );
    assert_eq!(members::role_of(&pool, b.id, pa.id).await.unwrap(), None);

    let m = members::add(&pool, a.id, pa.id, agent.id, Role::Agent, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            m.user_id,
            m.role,
            m.should_notify,
            m.can_view_all_tickets,
            m.can_manage_templates
        ),
        (agent.id, Role::Agent, true, true, false)
    );
    assert_eq!(m.user_email, "agent@example.com");
    assert_eq!(
        members::role_of(&pool, agent.id, pa.id).await.unwrap(),
        Some(Role::Agent)
    );

    let list = members::list(&pool, agent.id, pa.id).await.unwrap();
    assert_eq!(list.len(), 2);
    assert!(
        members::list(&pool, b.id, pa.id).await.unwrap().is_empty(),
        "outsider sees nothing"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn only_project_admins_add_and_no_duplicates(pool: Pool) {
    let (a, pa, b, _pb) = two_tenants(&pool).await;
    let agent = user(&pool, "agent@example.com").await;
    let other = user(&pool, "other@example.com").await;
    add_member(&pool, a.id, pa.id, agent.id, Role::Agent).await;

    assert!(
        members::add(&pool, b.id, pa.id, other.id, Role::Agent, false)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        members::add(&pool, agent.id, pa.id, other.id, Role::Agent, false)
            .await
            .unwrap()
            .is_none()
    );
    let dup = members::add(&pool, a.id, pa.id, agent.id, Role::Admin, false)
        .await
        .unwrap_err();
    assert!(is_unique_violation(&dup));
}

#[sqlx::test(migrations = "./migrations")]
async fn search_matches_name_or_email_case_insensitively(pool: Pool) {
    let (a, pa, b, _pb) = two_tenants(&pool).await;
    let anna = dp_db::users::create(&pool, "anna@corp.io", "Anna Smith", "h", Role::Agent)
        .await
        .unwrap();
    let bob = dp_db::users::create(&pool, "bob@corp.io", "Bob 100%", "h", Role::Agent)
        .await
        .unwrap();
    add_member(&pool, a.id, pa.id, anna.id, Role::Agent).await;
    add_member(&pool, a.id, pa.id, bob.id, Role::Agent).await;

    let by_name = members::search(&pool, a.id, pa.id, "ANNA", Page::new(50, 0))
        .await
        .unwrap();
    assert_eq!(by_name.len(), 1);
    let by_email = members::search(&pool, a.id, pa.id, "corp.io", Page::new(50, 0))
        .await
        .unwrap();
    assert_eq!(by_email.len(), 2);
    let all = members::search(&pool, a.id, pa.id, "", Page::new(50, 0))
        .await
        .unwrap();
    assert_eq!(all.len(), 3);
    // `%` is literal, not a wildcard.
    assert_eq!(
        members::search(&pool, a.id, pa.id, "100%", Page::new(50, 0))
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        members::search(&pool, a.id, pa.id, "%", Page::new(50, 0))
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        members::search(&pool, a.id, pa.id, "", Page::new(1, 1))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        members::search(&pool, b.id, pa.id, "", Page::new(50, 0))
            .await
            .unwrap()
            .is_empty()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn update_permissions_and_protect_the_owner(pool: Pool) {
    let (a, pa, b, _pb) = two_tenants(&pool).await;
    let agent = user(&pool, "agent@example.com").await;
    add_member(&pool, a.id, pa.id, agent.id, Role::Agent).await;

    let patch = MemberPatch {
        role: Some(Role::Admin),
        should_notify: Some(true),
        can_view_all_tickets: Some(false),
        can_manage_templates: Some(true),
    };
    assert!(
        members::update(&pool, b.id, pa.id, agent.id, &patch)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        members::update(&pool, agent.id, pa.id, agent.id, &patch)
            .await
            .unwrap()
            .is_none(),
        "agents cannot promote themselves"
    );
    let m = members::update(&pool, a.id, pa.id, agent.id, &patch)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            m.role,
            m.should_notify,
            m.can_view_all_tickets,
            m.can_manage_templates
        ),
        (Role::Admin, true, false, true)
    );

    // The owner cannot be demoted, but may change other flags.
    let demote = MemberPatch {
        role: Some(Role::Agent),
        ..Default::default()
    };
    assert!(
        members::update(&pool, agent.id, pa.id, a.id, &demote)
            .await
            .unwrap()
            .is_none()
    );
    let flags = MemberPatch {
        should_notify: Some(false),
        ..Default::default()
    };
    assert!(
        members::update(&pool, agent.id, pa.id, a.id, &flags)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        members::update(&pool, a.id, pa.id, dp_db::new_id(), &flags)
            .await
            .unwrap()
            .is_none()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn remove_rules_and_tickets_get_unassigned(pool: Pool) {
    let (a, pa, b, _pb) = two_tenants(&pool).await;
    let agent = user(&pool, "agent@example.com").await;
    let peer = user(&pool, "peer@example.com").await;
    add_member(&pool, a.id, pa.id, agent.id, Role::Agent).await;
    add_member(&pool, a.id, pa.id, peer.id, Role::Agent).await;

    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;
    let patch = tickets::TicketPatch {
        assignee: Some(Some(agent.id)),
        ..Default::default()
    };
    tickets::update(&pool, a.id, t.id, &patch)
        .await
        .unwrap()
        .unwrap();

    assert!(
        !members::remove(&pool, b.id, pa.id, agent.id).await.unwrap(),
        "outsider"
    );
    assert!(
        !members::remove(&pool, peer.id, pa.id, agent.id)
            .await
            .unwrap(),
        "agent removing someone else"
    );
    assert!(
        !members::remove(&pool, agent.id, pa.id, a.id).await.unwrap(),
        "owner is not removable"
    );
    assert!(
        !members::remove(&pool, a.id, pa.id, a.id).await.unwrap(),
        "not even by themself"
    );

    assert!(members::remove(&pool, a.id, pa.id, agent.id).await.unwrap());
    let after = tickets::get(&pool, a.id, t.id).await.unwrap().unwrap();
    assert_eq!(after.assigned_to_user_id, None);
    assert!(
        members::remove(&pool, peer.id, pa.id, peer.id)
            .await
            .unwrap(),
        "leaving on your own"
    );
    assert_eq!(members::role_of(&pool, peer.id, pa.id).await.unwrap(), None);
}

#[sqlx::test(migrations = "./migrations")]
async fn notify_recipients_are_flagged_active_members(pool: Pool) {
    let (a, pa, _b, _pb) = two_tenants(&pool).await;
    let quiet = user(&pool, "quiet@example.com").await;
    let loud = user(&pool, "loud@example.com").await;
    let gone = user(&pool, "gone@example.com").await;
    add_member(&pool, a.id, pa.id, quiet.id, Role::Agent).await;
    members::add(&pool, a.id, pa.id, loud.id, Role::Agent, true)
        .await
        .unwrap()
        .unwrap();
    members::add(&pool, a.id, pa.id, gone.id, Role::Agent, true)
        .await
        .unwrap()
        .unwrap();
    dp_db::users::admin_update(
        &pool,
        gone.id,
        &dp_db::users::AdminPatch {
            status: Some(dp_db::Status::Suspended),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let emails: Vec<_> = members::notify_recipients(&pool, pa.id)
        .await
        .unwrap()
        .into_iter()
        .map(|(e, _)| e)
        .collect();
    assert_eq!(
        emails,
        vec!["a@example.com".to_string(), "loud@example.com".to_string()]
    );
}
