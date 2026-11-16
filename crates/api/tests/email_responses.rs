mod common;
mod r21_world;

use axum::http::Method;
use r21_world::*;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

async fn make(w: &W, project: dp_db::Id, who: &Actor, status: &str) -> Value {
    let r = w
        .post("/email-responses", &who.token, json!({ "projectId": project, "status": status, "subject": "Got it", "body": "We got it." }))
        .await;
    assert_eq!(r.status, 201, "{}", r.text());
    data(&r)
}

#[sqlx::test(migrations = "../db/migrations")]
async fn create_is_admin_only_and_unique_per_status(pool: PgPool) {
    let w = world(pool).await;
    let body = json!({ "projectId": w.project.id, "status": "created", "body": "Hi" });
    assert_eq!(
        w.post("/email-responses", &w.agent.token, body.clone())
            .await
            .status,
        403
    );
    assert_eq!(
        w.post("/email-responses/", &w.other.token, body.clone())
            .await
            .status,
        404
    );
    assert_eq!(
        w.post("/email-responses", &w.root.token, body.clone())
            .await
            .status,
        404
    );
    assert_eq!(
        w.call(Method::POST, "/email-responses", None, Some(body.clone()))
            .await
            .status,
        401
    );

    let r = w
        .post("/email-responses", &w.admin.token, body.clone())
        .await;
    assert_eq!(r.status, 201);
    let d = data(&r);
    assert_eq!(
        (
            d["subject"].clone(),
            d["isEnabled"].clone(),
            d["status"].clone()
        ),
        (json!(""), json!(true), json!("created"))
    );
    let dup = w.post("/email-responses", &w.owner.token, body).await;
    assert_eq!(
        (dup.status.as_u16(), dup.error_code().as_str()),
        (409, "CONFLICT")
    );
    // The same status in another project is fine.
    make(&w, w.foreign.id, &w.other, "created").await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn create_validation(pool: PgPool) {
    let w = world(pool).await;
    let p = w.project.id;
    for bad in [
        json!({ "projectId": p, "status": "pending", "body": "b" }),
        json!({ "projectId": p, "status": "created", "body": "" }),
        json!({ "projectId": p, "status": "created", "body": "b".repeat(20_001) }),
        json!({ "projectId": p, "status": "created", "body": "b", "subject": "x".repeat(256) }),
        json!({ "projectId": p, "status": "created", "body": "b", "subject": "a\nBcc: x@y.zz" }),
        json!({ "status": "created", "body": "b" }),
        json!({ "projectId": p, "body": "b" }),
    ] {
        let r = w
            .post("/email-responses", &w.owner.token, bad.clone())
            .await;
        assert_eq!(r.status, 422, "{bad}");
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn list_filters_and_isolates(pool: PgPool) {
    let w = world(pool).await;
    make(&w, w.project.id, &w.owner, "created").await;
    make(&w, w.project.id, &w.owner, "resolved").await;
    make(&w, w.foreign.id, &w.other, "created").await;
    let q = format!("/email-responses/?projectId={}", w.project.id);
    assert_eq!(
        w.matrix(Method::GET, &q, None).await,
        [200, 200, 200, 404, 404]
    );
    assert_eq!(
        data(&w.get(&q, &w.agent.token).await)
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let only = data(&w.get(&format!("{q}&status=resolved"), &w.agent.token).await);
    assert_eq!(only.as_array().unwrap().len(), 1);
    for bad in [
        format!("{q}&status=nope"),
        "/email-responses/".to_string(),
        "/email-responses/?projectId=zz".to_string(),
    ] {
        assert_eq!(w.get(&bad, &w.owner.token).await.status, 422, "{bad}");
    }
    assert_eq!(
        w.get(
            &format!("/email-responses/?projectId={}", w.foreign.id),
            &w.owner.token
        )
        .await
        .status,
        404
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn update_and_delete_respect_roles_and_tenants(pool: PgPool) {
    let w = world(pool).await;
    let mine = make(&w, w.project.id, &w.owner, "created").await;
    let theirs = make(&w, w.foreign.id, &w.other, "created").await;
    let path = format!("/email-responses/{}", mine["id"].as_str().unwrap());
    let other_path = format!("/email-responses/{}", theirs["id"].as_str().unwrap());

    let body = Some(json!({ "isEnabled": false }));
    assert_eq!(
        w.matrix(Method::PATCH, &path, body).await,
        [200, 200, 403, 404, 404]
    );
    let r = w
        .patch(
            &path,
            &w.owner.token,
            json!({ "subject": null, "body": "Updated" }),
        )
        .await;
    let d = data(&r);
    assert_eq!(
        (
            d["subject"].clone(),
            d["body"].clone(),
            d["isEnabled"].clone()
        ),
        (json!(""), json!("Updated"), json!(false))
    );
    let r = w
        .patch(&path, &w.owner.token, json!({ "subject": "Re: you" }))
        .await;
    assert_eq!(data(&r)["subject"], "Re: you");
    let r = w
        .patch(&path, &w.owner.token, json!({ "body": "Kept" }))
        .await;
    assert_eq!(
        data(&r)["subject"],
        "Re: you",
        "omitted subject is unchanged"
    );
    for bad in [json!({ "body": "" }), json!({ "subject": "x".repeat(256) })] {
        assert_eq!(w.patch(&path, &w.owner.token, bad).await.status, 422);
    }
    // Foreign and unknown ids look the same.
    let a = w
        .patch(&other_path, &w.owner.token, json!({ "body": "pwn" }))
        .await;
    let b = w
        .patch(
            &format!("/email-responses/{}", Uuid::new_v4()),
            &w.owner.token,
            json!({ "body": "pwn" }),
        )
        .await;
    assert_eq!((a.status, a.json()), (b.status, b.json()));
    assert_eq!(w.delete(&other_path, &w.owner.token).await.status, 404);

    assert_eq!(
        w.matrix(Method::DELETE, &path, None).await,
        [200, 404, 404, 404, 404]
    );
    let left = w
        .get(
            &format!("/email-responses/?projectId={}", w.foreign.id),
            &w.other.token,
        )
        .await;
    assert_eq!(data(&left).as_array().unwrap().len(), 1);
}
