mod common;
mod tickets_fixture;

use std::time::Duration;

use axum::http::Method;
use serde_json::json;
use sqlx::PgPool;
use tickets_fixture::*;

async fn backdate(e: &Env, minutes: i32) {
    sqlx::query(
        "UPDATE active_ticket_viewers SET last_seen_at = now() - make_interval(mins => $1)",
    )
    .bind(minutes)
    .execute(&e.t.pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn heartbeat_list_and_leave(pool: PgPool) {
    let e = Env::new(pool).await;
    let id = e.ticket_id(&e.owner, "Presence").await;
    let path = format!("/viewers/{id}");
    assert_eq!(e.get(&path, &e.owner).await.json()["data"], json!([]));
    assert_eq!(
        e.post(&path, &e.owner, json!({})).await.json()["data"],
        json!({"success": true})
    );
    assert_eq!(e.post(&path, &e.agent, json!({})).await.status, 200);
    // idempotent heartbeat: still one row per user
    e.post(&path, &e.owner, json!({})).await;
    let r = e.get(&path, &e.agent).await;
    let mut users: Vec<_> = r.json()["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["userId"].as_str().unwrap().to_string())
        .collect();
    users.sort();
    let mut want = vec![e.owner_id.to_string(), e.agent_id.to_string()];
    want.sort();
    assert_eq!(users, want);
    assert_eq!(r.json()["data"][0]["name"], "Test User");
    let d = e.delete(&path, &e.owner).await;
    assert_eq!(d.json()["data"], json!({"success": true}));
    assert_eq!(
        e.get(&path, &e.agent).await.json()["data"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // leaving when not registered still succeeds
    assert_eq!(e.delete(&path, &e.owner).await.status, 200);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn viewers_tenant_and_auth(pool: PgPool) {
    let e = Env::new(pool).await;
    let id = e.ticket_id(&e.owner, "Presence tenant").await;
    let path = format!("/viewers/{id}");
    for m in [Method::GET, Method::POST, Method::DELETE] {
        assert_eq!(
            e.call(m.clone(), &path, None, None).await.status,
            401,
            "{m}"
        );
        let r = e.call(m.clone(), &path, Some(&e.outsider), None).await;
        assert_eq!(r.status, 404, "{m}");
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM active_ticket_viewers")
        .fetch_one(&e.t.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    assert_eq!(e.get("/viewers/nope", &e.owner).await.status, 400);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn expired_heartbeats_are_hidden_and_refreshable(pool: PgPool) {
    let e = Env::new(pool).await;
    let id = e.ticket_id(&e.owner, "Presence expiry").await;
    let path = format!("/viewers/{id}");
    e.post(&path, &e.owner, json!({})).await;
    backdate(&e, 1).await;
    assert_eq!(
        e.get(&path, &e.agent).await.json()["data"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    backdate(&e, 3).await;
    assert_eq!(e.get(&path, &e.agent).await.json()["data"], json!([]));
    e.post(&path, &e.owner, json!({})).await;
    assert_eq!(
        e.get(&path, &e.agent).await.json()["data"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn gc_task_purges_only_stale_rows(pool: PgPool) {
    let e = Env::new(pool).await;
    let id = e.ticket_id(&e.owner, "Presence gc").await;
    let path = format!("/viewers/{id}");
    e.post(&path, &e.owner, json!({})).await;
    backdate(&e, 10).await;
    e.post(&path, &e.agent, json!({})).await;
    let gc = api::routes::tickets::spawn_gc(e.t.pool.clone(), Duration::from_millis(20));
    let count = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM active_ticket_viewers")
            .fetch_one(&e.t.pool)
            .await
            .unwrap()
    };
    for _ in 0..100 {
        if count().await == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    gc.abort();
    assert_eq!(count().await, 1);
    assert_eq!(
        e.get(&path, &e.agent).await.json()["data"][0]["userId"],
        e.agent_id.to_string()
    );
}
