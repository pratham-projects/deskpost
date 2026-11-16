mod common;
mod r21_world;

use axum::http::Method;
use r21_world::*;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

async fn make(w: &W, project: dp_db::Id, who: &Actor, title: &str) -> Value {
    let r = w
        .post(
            "/template-responses/",
            &who.token,
            json!({ "projectId": project, "title": title, "body": format!("Body of {title}") }),
        )
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    data(&r)
}

#[sqlx::test(migrations = "../db/migrations")]
async fn create_needs_admin_or_the_templates_flag(pool: PgPool) {
    let w = world(pool).await;
    let body = json!({ "projectId": w.project.id, "title": "Greeting", "body": "Hello!" });
    assert_eq!(
        w.post("/template-responses", &w.agent.token, body.clone())
            .await
            .status,
        403
    );
    assert_eq!(
        w.post("/template-responses/", &w.other.token, body.clone())
            .await
            .status,
        404
    );
    assert_eq!(
        w.post("/template-responses/", &w.root.token, body.clone())
            .await
            .status,
        404
    );
    assert_eq!(
        w.call(
            Method::POST,
            "/template-responses/",
            None,
            Some(body.clone())
        )
        .await
        .status,
        401
    );

    let r = w
        .post("/template-responses/", &w.admin.token, body.clone())
        .await;
    assert_eq!(r.status, 201);
    let d = data(&r);
    assert_eq!(
        (d["title"].clone(), d["projectId"].clone()),
        (json!("Greeting"), json!(w.project.id))
    );

    // The flag opens it for an agent.
    dp_db::members::update(
        &w.t.pool,
        w.owner.id,
        w.project.id,
        w.agent.id,
        &dp_db::members::MemberPatch {
            can_manage_templates: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        w.post("/template-responses/", &w.agent.token, body)
            .await
            .status,
        201
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn create_validation(pool: PgPool) {
    let w = world(pool).await;
    let p = w.project.id;
    let res = w
        .post(
            "/template-responses/",
            &w.owner.token,
            json!({ "projectId": p, "title": "", "body": "  " }),
        )
        .await;
    assert_eq!(res.status, 422);
    let details = &res.json()["error"]["details"];
    assert!(details.get("title").is_some() && details.get("body").is_some());
    for bad in [
        json!({ "projectId": p, "title": "x".repeat(256), "body": "b" }),
        json!({ "projectId": p, "title": "t", "body": "b".repeat(20_001) }),
        json!({ "title": "t", "body": "b" }),
        json!({ "projectId": "nope", "title": "t", "body": "b" }),
    ] {
        assert_eq!(
            w.post("/template-responses/", &w.owner.token, bad)
                .await
                .status,
            422
        );
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn list_search_and_isolation(pool: PgPool) {
    let w = world(pool).await;
    make(&w, w.project.id, &w.owner, "Refund policy").await;
    make(&w, w.project.id, &w.owner, "Welcome").await;
    make(&w, w.foreign.id, &w.other, "Secret of B").await;

    let q = format!("/template-responses/?projectId={}", w.project.id);
    assert_eq!(
        w.matrix(Method::GET, &q, None).await,
        [200, 200, 200, 404, 404]
    );
    let list = w.get(&q, &w.agent.token).await;
    let titles: Vec<_> = data(&list)
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        titles,
        ["Refund policy", "Welcome"],
        "ordered by title, only this project"
    );
    assert_eq!(list.json()["pagination"]["total"], 2);
    assert!(!list.text().contains("Secret of B"));

    let s = format!(
        "/template-responses/search?projectId={}&q=REFUND",
        w.project.id
    );
    assert_eq!(
        data(&w.get(&s, &w.agent.token).await)
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let by_body = format!(
        "/template-responses/search?projectId={}&q=body%20of%20wel",
        w.project.id
    );
    assert_eq!(
        data(&w.get(&by_body, &w.agent.token).await)
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let wild = format!(
        "/template-responses/search?projectId={}&q=%25",
        w.project.id
    );
    assert_eq!(data(&w.get(&wild, &w.agent.token).await), json!([]));
    assert_eq!(
        w.get(
            &format!("/template-responses/search?projectId={}", w.project.id),
            &w.root.token
        )
        .await
        .status,
        404
    );

    for bad in [
        "/template-responses/",
        "/template-responses/?projectId=x",
        &format!("{q}&limit=0"),
    ] {
        assert_eq!(w.get(bad, &w.owner.token).await.status, 422, "{bad}");
    }
    let mine = format!(
        "/template-responses/?projectId={}&limit=1&offset=1",
        w.project.id
    );
    assert_eq!(
        w.get(&mine, &w.owner.token).await.json()["pagination"]["hasMore"],
        false
    );
    // The other tenant's id is 404 for tenant A, and vice versa.
    let theirs = format!("/template-responses/?projectId={}", w.foreign.id);
    assert_eq!(w.get(&theirs, &w.owner.token).await.status, 404);
    assert_eq!(
        w.get(
            &format!("/template-responses/?projectId={}", Uuid::new_v4()),
            &w.owner.token
        )
        .await
        .status,
        404
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn update_and_delete_by_id_respect_roles_and_tenants(pool: PgPool) {
    let w = world(pool).await;
    let t = make(&w, w.project.id, &w.owner, "Original").await;
    let foreign = make(&w, w.foreign.id, &w.other, "Theirs").await;
    let path = format!("/template-responses/{}", t["id"].as_str().unwrap());
    let theirs = format!("/template-responses/{}", foreign["id"].as_str().unwrap());

    let body = Some(json!({ "title": "Edited" }));
    // owner, admin ok; agent without the flag 403; outsiders 404.
    assert_eq!(
        w.matrix(Method::PATCH, &path, body).await,
        [200, 200, 403, 404, 404]
    );
    let r = w
        .patch(&path, &w.owner.token, json!({ "body": "New body" }))
        .await;
    assert_eq!(
        (data(&r)["title"].clone(), data(&r)["body"].clone()),
        (json!("Edited"), json!("New body"))
    );
    for bad in [
        json!({ "title": "" }),
        json!({ "body": "" }),
        json!({ "title": "x".repeat(256) }),
    ] {
        assert_eq!(w.patch(&path, &w.owner.token, bad).await.status, 422);
    }
    // Tenant A cannot touch B's template, it looks missing.
    let missing = format!("/template-responses/{}", Uuid::new_v4());
    let cross = w
        .patch(&theirs, &w.owner.token, json!({ "title": "pwn" }))
        .await;
    let gone = w
        .patch(&missing, &w.owner.token, json!({ "title": "pwn" }))
        .await;
    assert_eq!((cross.status, cross.json()), (gone.status, gone.json()));
    assert_eq!(w.delete(&theirs, &w.admin.token).await.status, 404);

    assert_eq!(
        w.matrix(Method::DELETE, &path, None).await,
        [200, 404, 404, 404, 404]
    );
    let still = w
        .get(
            &format!("/template-responses/?projectId={}", w.foreign.id),
            &w.other.token,
        )
        .await;
    assert_eq!(
        data(&still)[0]["title"],
        "Theirs",
        "tenant B was never touched"
    );
}
