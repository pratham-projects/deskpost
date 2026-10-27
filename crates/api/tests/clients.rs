mod common;
mod r21_world;

use axum::http::Method;
use r21_world::*;
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrations = "../db/migrations")]
async fn clients_are_listed_per_project_for_members(pool: PgPool) {
    let w = world(pool).await;
    let mine = sd_db::clients::upsert(&w.t.pool, w.project.id, "Alice@Client.com", Some("Alice"))
        .await
        .unwrap();
    sd_db::clients::upsert(&w.t.pool, w.project.id, "bob@client.com", None)
        .await
        .unwrap();
    let theirs = sd_db::clients::upsert(
        &w.t.pool,
        w.foreign.id,
        "alice@client.com",
        Some("Other Alice"),
    )
    .await
    .unwrap();

    let q = format!("/clients?projectId={}", w.project.id);
    assert_eq!(
        w.matrix(Method::GET, &q, None).await,
        [200, 200, 200, 404, 404]
    );
    let list = w.get(&q, &w.agent.token).await;
    assert_eq!(data(&list).as_array().unwrap().len(), 2);
    assert!(!list.text().contains("Other Alice"));
    let hit = data(&w.get(&format!("{q}&q=ALICE"), &w.agent.token).await);
    assert_eq!(
        (hit.as_array().unwrap().len(), hit[0]["email"].clone()),
        (1, serde_json::json!("Alice@Client.com"))
    );
    for bad in [
        "/clients",
        "/clients?projectId=x",
        &format!("{q}&limit=999"),
    ] {
        assert_eq!(w.get(bad, &w.owner.token).await.status, 422, "{bad}");
    }
    assert_eq!(
        w.get(
            &format!("/clients/?projectId={}", w.foreign.id),
            &w.owner.token
        )
        .await
        .status,
        404
    );

    let one = |c: &sd_db::clients::Client| format!("/clients/{}", c.id);
    assert_eq!(
        w.matrix(Method::GET, &one(&mine), None).await,
        [200, 200, 200, 404, 404]
    );
    let cross = w.get(&one(&theirs), &w.owner.token).await;
    let gone = w
        .get(&format!("/clients/{}", Uuid::new_v4()), &w.owner.token)
        .await;
    assert_eq!((cross.status, cross.json()), (gone.status, gone.json()));
}
