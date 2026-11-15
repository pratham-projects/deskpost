mod common;

use chrono::{Duration, Utc};
use common::*;
use dp_db::{Pool, resets, sessions, users};

async fn live_families(pool: &Pool, user: dp_db::Id) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM refresh_families WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn session(pool: &Pool, user: dp_db::Id, hash: &str) {
    let now = Utc::now();
    sessions::start_family(
        pool,
        user,
        hash,
        now + Duration::days(7),
        now + Duration::days(90),
        &Default::default(),
    )
    .await
    .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn token_is_single_use_and_sets_the_password(pool: Pool) {
    let u = user(&pool, "a@x.test").await;
    resets::create(&pool, u.id, "h1", Utc::now() + Duration::hours(1))
        .await
        .unwrap();
    let email = resets::consume(&pool, "h1", "new-hash").await.unwrap();
    assert_eq!(email.as_deref(), Some("a@x.test"));
    assert_eq!(
        users::get(&pool, u.id)
            .await
            .unwrap()
            .unwrap()
            .password_hash,
        "new-hash"
    );
    // Replay with the same token changes nothing.
    assert!(
        resets::consume(&pool, "h1", "evil-hash")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        users::get(&pool, u.id)
            .await
            .unwrap()
            .unwrap()
            .password_hash,
        "new-hash"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn expired_and_unknown_tokens_are_refused(pool: Pool) {
    let u = user(&pool, "a@x.test").await;
    resets::create(&pool, u.id, "old", Utc::now() - Duration::seconds(1))
        .await
        .unwrap();
    assert!(resets::consume(&pool, "old", "h").await.unwrap().is_none());
    assert!(resets::consume(&pool, "nope", "h").await.unwrap().is_none());
    assert_eq!(
        users::get(&pool, u.id)
            .await
            .unwrap()
            .unwrap()
            .password_hash,
        "hash"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_new_request_replaces_the_old_token(pool: Pool) {
    let u = user(&pool, "a@x.test").await;
    let exp = Utc::now() + Duration::hours(1);
    resets::create(&pool, u.id, "first", exp).await.unwrap();
    resets::create(&pool, u.id, "second", exp).await.unwrap();
    assert!(
        resets::consume(&pool, "first", "h")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        resets::consume(&pool, "second", "h")
            .await
            .unwrap()
            .is_some()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn consuming_revokes_every_session(pool: Pool) {
    let u = user(&pool, "a@x.test").await;
    session(&pool, u.id, "t1").await;
    session(&pool, u.id, "t2").await;
    assert_eq!(live_families(&pool, u.id).await, 2);
    resets::create(&pool, u.id, "h1", Utc::now() + Duration::hours(1))
        .await
        .unwrap();
    resets::consume(&pool, "h1", "h").await.unwrap().unwrap();
    assert_eq!(live_families(&pool, u.id).await, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn inactive_accounts_cannot_reset(pool: Pool) {
    let u = user(&pool, "a@x.test").await;
    sqlx::query("UPDATE users SET status = 'inactive' WHERE id = $1")
        .bind(u.id)
        .execute(&pool)
        .await
        .unwrap();
    resets::create(&pool, u.id, "h1", Utc::now() + Duration::hours(1))
        .await
        .unwrap();
    assert!(resets::consume(&pool, "h1", "h").await.unwrap().is_none());
}

#[sqlx::test(migrations = "./migrations")]
async fn a_password_change_kills_outstanding_links(pool: Pool) {
    let u = user(&pool, "a@x.test").await;
    resets::create(&pool, u.id, "h1", Utc::now() + Duration::hours(1))
        .await
        .unwrap();
    users::set_password_hash(&pool, u.id, "changed")
        .await
        .unwrap();
    assert!(resets::consume(&pool, "h1", "h").await.unwrap().is_none());
}
