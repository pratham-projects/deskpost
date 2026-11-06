mod common;

use common::*;
use sd_db::projects::{MailUpdate, NewProject, ProjectPatch};
use sd_db::{Page, Pool, Role, Status, constraint, is_unique_violation, projects};

fn new_project(slug: &str, hash: &str) -> NewProject {
    NewProject {
        name: "P".into(),
        slug: slug.into(),
        domain: None,
        description: None,
        api_key_hash: hash.into(),
        api_key_prefix: "pk_abcd".into(),
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn create_makes_owner_a_full_admin_member(pool: Pool) {
    let owner = user(&pool, "o@example.com").await;
    let p = project(&pool, owner.id, "acme").await;
    assert_eq!(
        (
            p.owner_user_id,
            p.status,
            p.smtp_port,
            p.imap_folder.as_str()
        ),
        (owner.id, Status::Active, 587, "INBOX")
    );
    assert!(!p.has_smtp_password && !p.has_imap_password);
    let m = sd_db::members::list(&pool, owner.id, p.id).await.unwrap();
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].role, Role::Admin);
    assert!(m[0].should_notify && m[0].can_view_all_tickets && m[0].can_manage_templates);
}

#[sqlx::test(migrations = "./migrations")]
async fn unique_slug_domain_key_and_mailbox(pool: Pool) {
    let o = user(&pool, "o@example.com").await;
    let mut np = new_project("acme", "h1");
    np.domain = Some("Acme.com".into());
    let p = projects::create(&pool, o.id, &np).await.unwrap();

    let slug = projects::create(&pool, o.id, &new_project("acme", "h2"))
        .await
        .unwrap_err();
    assert_eq!(constraint(&slug), Some("projects_slug_key"));

    let mut dom = new_project("other", "h3");
    dom.domain = Some("acme.COM".into());
    let e = projects::create(&pool, o.id, &dom).await.unwrap_err();
    assert_eq!(constraint(&e), Some("projects_domain_key"));

    let key = projects::create(&pool, o.id, &new_project("third", "h1"))
        .await
        .unwrap_err();
    assert!(is_unique_violation(&key));

    // Failed creates must not leave a half-made project behind.
    assert_eq!(count(&pool, "projects").await, 1);

    let mail = MailUpdate {
        smtp_email: Some("Support@Acme.com".into()),
        ..Default::default()
    };
    projects::set_mail_settings(&pool, o.id, p.id, &mail)
        .await
        .unwrap()
        .unwrap();
    let o2 = user(&pool, "o2@example.com").await;
    let q = project(&pool, o2.id, "beta").await;
    let clash = MailUpdate {
        smtp_email: Some("support@acme.com".into()),
        ..Default::default()
    };
    let e = projects::set_mail_settings(&pool, o2.id, q.id, &clash)
        .await
        .unwrap_err();
    assert_eq!(constraint(&e), Some("projects_smtp_email_key"));
}

#[sqlx::test(migrations = "./migrations")]
async fn bad_slug_and_port_are_rejected(pool: Pool) {
    let o = user(&pool, "o@example.com").await;
    for slug in ["ab", "Has Caps", "under_score"] {
        assert!(
            projects::create(&pool, o.id, &new_project(slug, slug))
                .await
                .is_err(),
            "{slug}"
        );
    }
    let p = project(&pool, o.id, "ok-slug").await;
    let bad_port = MailUpdate {
        smtp_port: Some(70000),
        ..Default::default()
    };
    assert!(
        projects::set_mail_settings(&pool, o.id, p.id, &bad_port)
            .await
            .is_err()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn list_and_get_only_show_member_projects(pool: Pool) {
    let (a, pa, b, pb) = two_tenants(&pool).await;
    let (rows, total) = projects::list(&pool, a.id, None, Page::new(50, 0))
        .await
        .unwrap();
    assert_eq!((rows.len(), total, rows[0].id), (1, 1, pa.id));
    assert!(projects::get(&pool, a.id, pa.id).await.unwrap().is_some());
    assert!(projects::get(&pool, a.id, pb.id).await.unwrap().is_none());

    add_member(&pool, b.id, pb.id, a.id, Role::Agent).await;
    let (rows, total) = projects::list(&pool, a.id, None, Page::new(50, 0))
        .await
        .unwrap();
    assert_eq!((rows.len(), total), (2, 2));
    let (rows, _) = projects::list(&pool, a.id, None, Page::new(1, 1))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);

    projects::update(
        &pool,
        b.id,
        pb.id,
        &ProjectPatch {
            status: Some(Status::Inactive),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let (rows, _) = projects::list(&pool, a.id, Some(Status::Inactive), Page::new(50, 0))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    let (rows, _) = projects::list(&pool, a.id, Some(Status::Active), Page::new(50, 0))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn update_needs_a_project_admin(pool: Pool) {
    let (a, pa, b, _pb) = two_tenants(&pool).await;
    let patch = ProjectPatch {
        name: Some("Renamed".into()),
        domain: Some("x.com".into()),
        ..Default::default()
    };
    // Outsider and plain agent are both refused.
    assert!(
        projects::update(&pool, b.id, pa.id, &patch)
            .await
            .unwrap()
            .is_none()
    );
    add_member(&pool, a.id, pa.id, b.id, Role::Agent).await;
    assert!(
        projects::update(&pool, b.id, pa.id, &patch)
            .await
            .unwrap()
            .is_none()
    );

    let ok = projects::update(&pool, a.id, pa.id, &patch)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (ok.name.as_str(), ok.domain.as_deref()),
        ("Renamed", Some("x.com"))
    );
    let cleared = projects::update(
        &pool,
        a.id,
        pa.id,
        &ProjectPatch {
            domain: Some(String::new()),
            description: Some("d".into()),
            is_archived: Some(true),
            auto_close_days: Some(3),
            slug: Some("new-slug".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(cleared.domain, None);
    assert_eq!(
        (
            cleared.description.as_deref(),
            cleared.is_archived,
            cleared.auto_close_days,
            cleared.slug.as_str()
        ),
        (Some("d"), true, 3, "new-slug")
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn mail_settings_roundtrip_and_never_leak_through_project(pool: Pool) {
    let (a, pa, b, _pb) = two_tenants(&pool).await;
    let upd = MailUpdate {
        reply_to_email: Some("reply@acme.com".into()),
        smtp_email: Some("help@acme.com".into()),
        smtp_username: Some("u".into()),
        smtp_password_encrypted: Some("CIPHER-SMTP".into()),
        smtp_host: Some("smtp.acme.com".into()),
        smtp_port: Some(465),
        imap_host: Some("imap.acme.com".into()),
        imap_port: Some(993),
        imap_username: Some("iu".into()),
        imap_password_encrypted: Some("CIPHER-IMAP".into()),
        imap_folder: Some("Support".into()),
    };
    let p = projects::set_mail_settings(&pool, a.id, pa.id, &upd)
        .await
        .unwrap()
        .unwrap();
    assert!(p.has_smtp_password && p.has_imap_password);
    assert_eq!(
        (p.smtp_host.as_deref(), p.smtp_port, p.imap_folder.as_str()),
        (Some("smtp.acme.com"), 465, "Support")
    );
    assert!(!format!("{p:?}").contains("CIPHER"));

    let s = projects::get_mail_settings(&pool, a.id, pa.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.smtp_password_encrypted.as_deref(), Some("CIPHER-SMTP"));
    assert_eq!(s.imap_password_encrypted.as_deref(), Some("CIPHER-IMAP"));
    assert!(
        !format!("{s:?}").contains("CIPHER"),
        "Debug must not print ciphertext"
    );
    assert!(
        projects::get_mail_settings(&pool, b.id, pa.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        projects::set_mail_settings(&pool, b.id, pa.id, &upd)
            .await
            .unwrap()
            .is_none()
    );

    // None keeps, "" clears.
    let kept = projects::set_mail_settings(
        &pool,
        a.id,
        pa.id,
        &MailUpdate {
            smtp_port: Some(25),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert!(kept.has_smtp_password && kept.smtp_host.is_some());
    let cleared = projects::set_mail_settings(
        &pool,
        a.id,
        pa.id,
        &MailUpdate {
            smtp_password_encrypted: Some(String::new()),
            imap_host: Some(String::new()),
            imap_folder: Some(String::new()),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!cleared.has_smtp_password && cleared.imap_host.is_none());
    assert_eq!(
        cleared.imap_folder, "Support",
        "empty folder keeps the previous one"
    );

    assert_eq!(
        projects::mail_settings(&pool, pa.id)
            .await
            .unwrap()
            .unwrap()
            .smtp_port,
        25
    );
    assert!(
        projects::mail_settings(&pool, sd_db::new_id())
            .await
            .unwrap()
            .is_none()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn api_key_rotation_and_lookup(pool: Pool) {
    let (a, pa, b, _pb) = two_tenants(&pool).await;
    assert_eq!(
        projects::find_by_api_key_hash(&pool, "hash-tenant-a")
            .await
            .unwrap(),
        Some(pa.id)
    );
    assert!(
        projects::replace_api_key(&pool, b.id, pa.id, "evil", "pk_evil")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        projects::replace_api_key(&pool, a.id, pa.id, "fresh", "pk_new1")
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        projects::find_by_api_key_hash(&pool, "hash-tenant-a")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        projects::find_by_api_key_hash(&pool, "fresh")
            .await
            .unwrap(),
        Some(pa.id)
    );
    assert_eq!(
        projects::get(&pool, a.id, pa.id)
            .await
            .unwrap()
            .unwrap()
            .api_key_prefix,
        "pk_new1"
    );

    // Inactive projects do not authenticate the widget.
    projects::update(
        &pool,
        a.id,
        pa.id,
        &ProjectPatch {
            status: Some(Status::Suspended),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        projects::find_by_api_key_hash(&pool, "fresh")
            .await
            .unwrap(),
        None
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn delete_is_owner_only_and_cascades_everything(pool: Pool) {
    let (a, pa, b, _pb) = two_tenants(&pool).await;
    let admin = user(&pool, "admin@example.com").await;
    add_member(&pool, a.id, pa.id, admin.id, Role::Admin).await;
    let t = ticket(&pool, a.id, pa.id, "c@example.com").await;
    sd_db::templates::create(&pool, a.id, pa.id, "T", "B")
        .await
        .unwrap()
        .unwrap();
    sd_db::email_responses::create(
        &pool,
        a.id,
        pa.id,
        sd_db::EmailResponseStatus::Created,
        "",
        "hi",
        true,
    )
    .await
    .unwrap()
    .unwrap();
    sd_db::viewers::touch(&pool, a.id, t.id).await.unwrap();

    assert!(!projects::delete(&pool, b.id, pa.id).await.unwrap());
    assert!(
        !projects::delete(&pool, admin.id, pa.id).await.unwrap(),
        "admins that are not the owner cannot delete"
    );
    assert!(projects::delete(&pool, a.id, pa.id).await.unwrap());
    for table in [
        "tickets",
        "messages",
        "clients",
        "unread_tickets",
        "templates",
        "email_responses",
        "active_ticket_viewers",
    ] {
        assert_eq!(count(&pool, table).await, 0, "{table}");
    }
    assert_eq!(
        count(&pool, "project_members").await,
        1,
        "only tenant b's owner remains"
    );
    assert_eq!(count(&pool, "projects").await, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn inbound_routing_and_imap_cursor(pool: Pool) {
    let (a, pa, _b, _pb) = two_tenants(&pool).await;
    assert!(
        projects::find_by_smtp_email(&pool, "help@acme.com")
            .await
            .unwrap()
            .is_none()
    );
    assert!(projects::list_imap_enabled(&pool).await.unwrap().is_empty());

    let upd = MailUpdate {
        smtp_email: Some("help@acme.com".into()),
        imap_host: Some("imap.acme.com".into()),
        imap_port: Some(993),
        imap_username: Some("u".into()),
        imap_password_encrypted: Some("C".into()),
        ..Default::default()
    };
    projects::set_mail_settings(&pool, a.id, pa.id, &upd)
        .await
        .unwrap()
        .unwrap();
    assert!(
        projects::find_by_smtp_email(&pool, "HELP@acme.com")
            .await
            .unwrap()
            .is_none(),
        "unverified mailboxes do not route"
    );
    projects::mark_mail_verified(&pool, pa.id).await.unwrap();
    assert_eq!(
        projects::find_by_smtp_email(&pool, "HELP@acme.com")
            .await
            .unwrap(),
        Some(pa.id)
    );

    let enabled = projects::list_imap_enabled(&pool).await.unwrap();
    assert_eq!(enabled.len(), 1);
    assert_eq!(
        (
            enabled[0].project_id,
            enabled[0].imap_last_uid,
            enabled[0].imap_uid_validity
        ),
        (pa.id, 0, None)
    );

    assert!(
        projects::set_imap_cursor(&pool, pa.id, 77, 1234)
            .await
            .unwrap()
    );
    let s = projects::mail_settings(&pool, pa.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((s.imap_uid_validity, s.imap_last_uid), (Some(77), 1234));
    assert!(
        !projects::set_imap_cursor(&pool, sd_db::new_id(), 1, 1)
            .await
            .unwrap()
    );

    // Archived projects neither route inbound mail nor get polled.
    projects::update(
        &pool,
        a.id,
        pa.id,
        &ProjectPatch {
            is_archived: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(projects::list_imap_enabled(&pool).await.unwrap().is_empty());
    assert_eq!(
        projects::find_by_smtp_email(&pool, "help@acme.com")
            .await
            .unwrap(),
        None
    );
}
