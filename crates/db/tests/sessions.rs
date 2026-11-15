mod common;

use chrono::{Duration, Utc};
use common::*;
use dp_db::sessions::{ClientMeta, Rotation, rotate, start_family};
use dp_db::{Pool, sessions};

fn meta() -> ClientMeta {
    ClientMeta {
        ip_address: Some("203.0.113.9".into()),
        user_agent: Some("test".into()),
    }
}

async fn start(pool: &Pool, user: dp_db::Id, hash: &str) -> dp_db::Id {
    let now = Utc::now();
    start_family(
        pool,
        user,
        hash,
        now + Duration::days(7),
        now + Duration::days(90),
        &meta(),
    )
    .await
    .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn rotation_issues_a_new_token_and_consumes_the_old(pool: Pool) {
    let u = user(&pool, "u@example.com").await;
    let family = start(&pool, u.id, "t1").await;
    let out = rotate(
        &pool,
        "t1",
        "t2",
        Utc::now() + Duration::days(7),
        Duration::seconds(10),
        &meta(),
    )
    .await
    .unwrap();
    assert_eq!(
        out,
        Rotation::Rotated {
            user_id: u.id,
            family_id: family
        }
    );
    // The new token rotates again.
    let out = rotate(
        &pool,
        "t2",
        "t3",
        Utc::now() + Duration::days(7),
        Duration::seconds(10),
        &meta(),
    )
    .await
    .unwrap();
    assert!(matches!(out, Rotation::Rotated { .. }));
    assert_eq!(count(&pool, "refresh_tokens").await, 3);
}

#[sqlx::test(migrations = "./migrations")]
async fn replay_inside_grace_does_not_revoke(pool: Pool) {
    let u = user(&pool, "u@example.com").await;
    let family = start(&pool, u.id, "t1").await;
    rotate(
        &pool,
        "t1",
        "t2",
        Utc::now() + Duration::days(7),
        Duration::seconds(10),
        &meta(),
    )
    .await
    .unwrap();
    let again = rotate(
        &pool,
        "t1",
        "tx",
        Utc::now() + Duration::days(7),
        Duration::seconds(10),
        &meta(),
    )
    .await
    .unwrap();
    assert_eq!(again, Rotation::Grace);
    assert!(
        sessions::get_family(&pool, family)
            .await
            .unwrap()
            .unwrap()
            .revoked_at
            .is_none()
    );
    // The legitimately rotated token still works.
    let next = rotate(
        &pool,
        "t2",
        "t3",
        Utc::now() + Duration::days(7),
        Duration::seconds(10),
        &meta(),
    )
    .await
    .unwrap();
    assert!(matches!(next, Rotation::Rotated { .. }));
}

#[sqlx::test(migrations = "./migrations")]
async fn reuse_after_grace_revokes_the_whole_family(pool: Pool) {
    let u = user(&pool, "u@example.com").await;
    let family = start(&pool, u.id, "t1").await;
    rotate(
        &pool,
        "t1",
        "t2",
        Utc::now() + Duration::days(7),
        Duration::zero(),
        &meta(),
    )
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let stolen = rotate(
        &pool,
        "t1",
        "tx",
        Utc::now() + Duration::days(7),
        Duration::zero(),
        &meta(),
    )
    .await
    .unwrap();
    assert_eq!(stolen, Rotation::Reused);
    let fam = sessions::get_family(&pool, family).await.unwrap().unwrap();
    assert_eq!(fam.revoked_reason.as_deref(), Some("reuse_detected"));
    // The honest holder of the newest token is locked out too.
    let after = rotate(
        &pool,
        "t2",
        "t3",
        Utc::now() + Duration::days(7),
        Duration::zero(),
        &meta(),
    )
    .await
    .unwrap();
    assert_eq!(after, Rotation::Invalid);
}

#[sqlx::test(migrations = "./migrations")]
async fn unknown_and_expired_tokens_are_invalid(pool: Pool) {
    let u = user(&pool, "u@example.com").await;
    let g = Duration::seconds(10);
    assert_eq!(
        rotate(&pool, "nope", "n", Utc::now(), g, &meta())
            .await
            .unwrap(),
        Rotation::Invalid
    );

    let now = Utc::now();
    start_family(
        &pool,
        u.id,
        "old",
        now - Duration::seconds(1),
        now + Duration::days(90),
        &meta(),
    )
    .await
    .unwrap();
    assert_eq!(
        rotate(&pool, "old", "n", now + Duration::days(7), g, &meta())
            .await
            .unwrap(),
        Rotation::Invalid
    );

    start_family(
        &pool,
        u.id,
        "capped",
        now + Duration::days(7),
        now - Duration::seconds(1),
        &meta(),
    )
    .await
    .unwrap();
    assert_eq!(
        rotate(&pool, "capped", "n", now + Duration::days(7), g, &meta())
            .await
            .unwrap(),
        Rotation::Invalid
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn new_token_never_outlives_the_family_cap(pool: Pool) {
    let u = user(&pool, "u@example.com").await;
    let now = Utc::now();
    let cap = now + Duration::days(1);
    start_family(&pool, u.id, "t1", now + Duration::days(7), cap, &meta())
        .await
        .unwrap();
    rotate(
        &pool,
        "t1",
        "t2",
        now + Duration::days(7),
        Duration::seconds(10),
        &meta(),
    )
    .await
    .unwrap();
    let max: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT max(expires_at) FROM refresh_tokens")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(max <= cap + Duration::milliseconds(1));
}

#[sqlx::test(migrations = "./migrations")]
async fn sign_out_revokes_family_or_all_families(pool: Pool) {
    let u = user(&pool, "u@example.com").await;
    let g = Duration::seconds(10);
    let f1 = start(&pool, u.id, "a1").await;
    start(&pool, u.id, "b1").await;

    assert!(sessions::revoke_by_token(&pool, "a1").await.unwrap());
    assert!(
        !sessions::revoke_by_token(&pool, "a1").await.unwrap(),
        "already revoked"
    );
    assert!(!sessions::revoke_by_token(&pool, "unknown").await.unwrap());
    assert_eq!(
        sessions::get_family(&pool, f1)
            .await
            .unwrap()
            .unwrap()
            .revoked_reason
            .as_deref(),
        Some("logout")
    );
    assert_eq!(
        rotate(
            &pool,
            "a1",
            "a2",
            Utc::now() + Duration::days(7),
            g,
            &meta()
        )
        .await
        .unwrap(),
        Rotation::Invalid
    );

    assert_eq!(
        sessions::revoke_all_for_user(&pool, u.id, "logout_all")
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        rotate(
            &pool,
            "b1",
            "b2",
            Utc::now() + Duration::days(7),
            g,
            &meta()
        )
        .await
        .unwrap(),
        Rotation::Invalid
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn revoke_family_is_idempotent_and_purge_and_cascade_work(pool: Pool) {
    let u = user(&pool, "u@example.com").await;
    let f = start(&pool, u.id, "a1").await;
    assert!(
        sessions::revoke_family(&pool, f, "user_inactive")
            .await
            .unwrap()
    );
    assert!(!sessions::revoke_family(&pool, f, "logout").await.unwrap());
    assert!(
        sessions::get_family(&pool, dp_db::new_id())
            .await
            .unwrap()
            .is_none()
    );

    let now = Utc::now();
    start_family(&pool, u.id, "gone", now, now - Duration::days(1), &meta())
        .await
        .unwrap();
    assert_eq!(sessions::purge_expired(&pool).await.unwrap(), 1);
    assert_eq!(count(&pool, "refresh_families").await, 1);

    dp_db::users::delete(&pool, u.id).await.unwrap();
    assert_eq!(count(&pool, "refresh_families").await, 0);
    assert_eq!(count(&pool, "refresh_tokens").await, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_refresh_of_one_token_yields_a_single_winner(pool: Pool) {
    let u = user(&pool, "u@example.com").await;
    start(&pool, u.id, "t1").await;
    let mut set = tokio::task::JoinSet::new();
    for i in 0..6 {
        let pool = pool.clone();
        set.spawn(async move {
            rotate(
                &pool,
                "t1",
                &format!("n{i}"),
                Utc::now() + Duration::days(7),
                Duration::seconds(10),
                &ClientMeta::default(),
            )
            .await
            .unwrap()
        });
    }
    let (mut won, mut grace) = (0, 0);
    while let Some(r) = set.join_next().await {
        match r.unwrap() {
            Rotation::Rotated { .. } => won += 1,
            Rotation::Grace => grace += 1,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!((won, grace), (1, 5));
}
