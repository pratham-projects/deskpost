mod common;

use common::*;
use sd_db::users::{AdminPatch, ProfilePatch, UserFilter};
use sd_db::{
    Page, Pool, Role, Status, constraint, is_foreign_key_violation, is_unique_violation, users,
};

#[sqlx::test(migrations = "./migrations")]
async fn create_get_and_case_insensitive_lookup(pool: Pool) {
    let u = users::create(&pool, "Alice@Example.com", "Alice", "h", Role::Agent)
        .await
        .unwrap();
    assert_eq!(u.status, Status::Active);
    assert_eq!(u.timezone, "UTC");
    assert_eq!(
        users::get(&pool, u.id).await.unwrap().unwrap().email,
        "Alice@Example.com"
    );
    let by_email = users::get_by_email(&pool, "alice@EXAMPLE.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(by_email.id, u.id);
    assert!(
        users::get_by_email(&pool, "nobody@example.com")
            .await
            .unwrap()
            .is_none()
    );
    assert!(users::get(&pool, sd_db::new_id()).await.unwrap().is_none());
}

#[sqlx::test(migrations = "./migrations")]
async fn email_unique_ignoring_case(pool: Pool) {
    users::create(&pool, "a@example.com", "A", "h", Role::Agent)
        .await
        .unwrap();
    let err = users::create(&pool, "A@EXAMPLE.COM", "B", "h", Role::Agent)
        .await
        .unwrap_err();
    assert!(is_unique_violation(&err));
    assert_eq!(constraint(&err), Some("users_email_key"));
}

#[sqlx::test(migrations = "./migrations")]
async fn check_constraints_reject_bad_values(pool: Pool) {
    let bad_role = sqlx::query("INSERT INTO users (id, email, name, password_hash, role) VALUES ($1, 'x@e.com', 'x', 'h', 'root')")
        .bind(sd_db::new_id()).execute(&pool).await;
    assert!(bad_role.is_err());
    let bad_status = sqlx::query("INSERT INTO users (id, email, name, password_hash, status) VALUES ($1, 'y@e.com', 'y', 'h', 'banned')")
        .bind(sd_db::new_id()).execute(&pool).await;
    assert!(bad_status.is_err());
    let empty_name = sqlx::query(
        "INSERT INTO users (id, email, name, password_hash) VALUES ($1, 'z@e.com', '', 'h')",
    )
    .bind(sd_db::new_id())
    .execute(&pool)
    .await;
    assert!(empty_name.is_err());
}

#[sqlx::test(migrations = "./migrations")]
async fn register_first_user_is_admin_then_agents(pool: Pool) {
    let first = users::register(&pool, "first@example.com", "First", "h")
        .await
        .unwrap();
    let second = users::register(&pool, "second@example.com", "Second", "h")
        .await
        .unwrap();
    assert_eq!(first.role, Role::Admin);
    assert_eq!(second.role, Role::Agent);
    let dup = users::register(&pool, "FIRST@example.com", "Again", "h")
        .await
        .unwrap_err();
    assert!(is_unique_violation(&dup));
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_register_makes_exactly_one_admin(pool: Pool) {
    let mut set = tokio::task::JoinSet::new();
    for i in 0..8 {
        let pool = pool.clone();
        set.spawn(async move {
            users::register(&pool, &format!("u{i}@example.com"), "U", "h")
                .await
                .unwrap()
        });
    }
    let mut admins = 0;
    while let Some(u) = set.join_next().await {
        if u.unwrap().role == Role::Admin {
            admins += 1;
        }
    }
    assert_eq!(admins, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn list_and_get_visible_are_scoped_to_shared_projects(pool: Pool) {
    let (a, pa, b, _pb) = two_tenants(&pool).await;
    let colleague = user(&pool, "c@example.com").await;
    add_member(&pool, a.id, pa.id, colleague.id, Role::Agent).await;

    let (seen, total) = users::list(&pool, a.id, &UserFilter::default(), Page::new(50, 0))
        .await
        .unwrap();
    let ids: Vec<_> = seen.iter().map(|u| u.id).collect();
    assert_eq!(total, 2);
    assert!(ids.contains(&a.id) && ids.contains(&colleague.id) && !ids.contains(&b.id));

    assert!(
        users::get_visible(&pool, a.id, colleague.id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        users::get_visible(&pool, a.id, b.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        users::get_visible(&pool, a.id, a.id)
            .await
            .unwrap()
            .is_some()
    );

    // A global admin sees everyone.
    let root = users::create(&pool, "root@example.com", "Root", "h", Role::Admin)
        .await
        .unwrap();
    let (all, total) = users::list(&pool, root.id, &UserFilter::default(), Page::new(50, 0))
        .await
        .unwrap();
    assert_eq!((all.len(), total), (4, 4));
    assert!(
        users::get_visible(&pool, root.id, b.id)
            .await
            .unwrap()
            .is_some()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn list_filters_and_paginates(pool: Pool) {
    let root = users::create(&pool, "root@example.com", "Root", "h", Role::Admin)
        .await
        .unwrap();
    for i in 0..5 {
        user(&pool, &format!("u{i}@example.com")).await;
    }
    let only_admins = UserFilter {
        role: Some(Role::Admin),
        status: None,
    };
    let (rows, total) = users::list(&pool, root.id, &only_admins, Page::new(50, 0))
        .await
        .unwrap();
    assert_eq!((rows.len(), total), (1, 1));

    let (page, total) = users::list(&pool, root.id, &UserFilter::default(), Page::new(2, 4))
        .await
        .unwrap();
    assert_eq!((page.len(), total), (2, 6));
    let (page, _) = users::list(&pool, root.id, &UserFilter::default(), Page::new(2, 100))
        .await
        .unwrap();
    assert!(page.is_empty());

    users::admin_update(
        &pool,
        root.id,
        &AdminPatch {
            status: Some(Status::Suspended),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let suspended = UserFilter {
        role: None,
        status: Some(Status::Suspended),
    };
    let (rows, _) = users::list(&pool, root.id, &suspended, Page::new(50, 0))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn updates_login_password_and_delete(pool: Pool) {
    let u = user(&pool, "u@example.com").await;
    assert!(u.last_login_at.is_none());
    users::touch_login(&pool, u.id).await.unwrap();
    assert!(
        users::get(&pool, u.id)
            .await
            .unwrap()
            .unwrap()
            .last_login_at
            .is_some()
    );

    let p = users::update_profile(
        &pool,
        u.id,
        &ProfilePatch {
            name: Some("New".into()),
            phone: Some("555".into()),
            timezone: Some("Europe/Berlin".into()),
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        (p.name.as_str(), p.phone.as_deref(), p.timezone.as_str()),
        ("New", Some("555"), "Europe/Berlin")
    );
    let cleared = users::update_profile(
        &pool,
        u.id,
        &ProfilePatch {
            phone: Some(String::new()),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(cleared.phone, None);
    assert_eq!(cleared.name, "New");
    assert!(
        users::update_profile(&pool, sd_db::new_id(), &ProfilePatch::default())
            .await
            .unwrap()
            .is_none()
    );

    let a = users::admin_update(
        &pool,
        u.id,
        &AdminPatch {
            role: Some(Role::Admin),
            status: Some(Status::Inactive),
            name: Some("Boss".into()),
            phone: Some("1".into()),
            timezone: Some("UTC".into()),
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        (a.role, a.status, a.name.as_str()),
        (Role::Admin, Status::Inactive, "Boss")
    );
    assert!(
        users::admin_update(&pool, sd_db::new_id(), &AdminPatch::default())
            .await
            .unwrap()
            .is_none()
    );

    assert!(
        users::set_password_hash(&pool, u.id, "new-hash")
            .await
            .unwrap()
    );
    assert_eq!(
        users::get(&pool, u.id)
            .await
            .unwrap()
            .unwrap()
            .password_hash,
        "new-hash"
    );
    assert!(
        !users::set_password_hash(&pool, sd_db::new_id(), "x")
            .await
            .unwrap()
    );

    assert!(users::delete(&pool, u.id).await.unwrap());
    assert!(!users::delete(&pool, u.id).await.unwrap());
}

#[sqlx::test(migrations = "./migrations")]
async fn cannot_delete_a_user_who_owns_a_project(pool: Pool) {
    let (a, _pa, _b, _pb) = two_tenants(&pool).await;
    let err = users::delete(&pool, a.id).await.unwrap_err();
    assert!(is_foreign_key_violation(&err));
}
