mod common;
mod r21_world;

use axum::http::Method;
use dp_db::Role;
use r21_world::*;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

fn base(w: &W) -> String {
    format!("/projects/{}/members", w.project.id)
}

#[sqlx::test(migrations = "../db/migrations")]
async fn list_and_search_are_member_only(pool: PgPool) {
    let w = world(pool).await;
    assert_eq!(
        w.matrix(Method::GET, &base(&w), None).await,
        [200, 200, 200, 404, 404]
    );
    let list = w.get(&base(&w), &w.agent.token).await;
    let rows = data(&list);
    assert_eq!(rows.as_array().unwrap().len(), 3);
    assert_eq!(rows[0]["user"]["email"], "owner@example.com");
    assert!(rows[0]["user"].get("passwordHash").is_none());
    assert_no_secrets(&list, &[]);

    let q = format!("{}/search?q=ADMIN@", base(&w));
    assert_eq!(
        w.matrix(Method::GET, &q, None).await,
        [200, 200, 200, 404, 404]
    );
    let hits = data(&w.get(&q, &w.agent.token).await);
    assert_eq!(hits.as_array().unwrap().len(), 1);
    assert_eq!(hits[0]["user"]["email"], "admin@example.com");
    let all = data(&w.get(&format!("{}/search", base(&w)), &w.agent.token).await);
    assert_eq!(all.as_array().unwrap().len(), 3, "empty q matches everyone");
    // `%` is literal, not a wildcard.
    let none = data(
        &w.get(&format!("{}/search?q=%25", base(&w)), &w.agent.token)
            .await,
    );
    assert_eq!(none, json!([]));
    assert_eq!(
        w.get(&format!("{}/search?limit=0", base(&w)), &w.agent.token)
            .await
            .status,
        422
    );
}

/// A user the project admins can see because they share another project with them.
async fn visible_user(w: &W, email: &str, slug: &str) -> dp_db::users::User {
    let u = w.t.user(email, Role::Agent).await;
    let side = w.t.project(w.owner.id, slug).await;
    w.t.add_member(w.owner.id, side.id, w.admin.id, Role::Agent)
        .await;
    w.t.add_member(w.owner.id, side.id, u.id, Role::Agent).await;
    u
}

#[sqlx::test(migrations = "../db/migrations")]
async fn add_member_needs_a_visible_active_user(pool: PgPool) {
    let w = world(pool).await;
    // Shares no project with the actor: indistinguishable from an unknown id.
    let stranger = w.t.user("stranger@example.com", Role::Agent).await;
    let probe = w
        .post(&base(&w), &w.owner.token, json!({ "userId": stranger.id }))
        .await;
    let unknown = w
        .post(
            &base(&w),
            &w.owner.token,
            json!({ "userId": Uuid::new_v4() }),
        )
        .await;
    assert_eq!((probe.status.as_u16(), unknown.status.as_u16()), (404, 404));
    assert_eq!(probe.text(), unknown.text(), "no oracle for user ids");
    assert!(!probe.text().contains("stranger@example.com"));
    // Visible but suspended: refused too.
    let gone = visible_user(&w, "gone@example.com", "side-a").await;
    sqlx::query("UPDATE users SET status = 'suspended' WHERE id = $1")
        .bind(gone.id)
        .execute(&w.t.pool)
        .await
        .unwrap();
    let r = w
        .post(&base(&w), &w.owner.token, json!({ "userId": gone.id }))
        .await;
    assert_eq!(r.status, 404);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn add_member_role_matrix_and_validation(pool: PgPool) {
    let w = world(pool).await;
    let newbie = visible_user(&w, "new@example.com", "side-n").await;
    let body = json!({ "userId": newbie.id });
    // Agents are refused, outsiders get 404, and a refused add changes nothing.
    for (who, want) in [(&w.agent, 403), (&w.other, 404), (&w.root, 404)] {
        assert_eq!(
            w.post(&base(&w), &who.token, body.clone()).await.status,
            want
        );
    }
    let res = w
        .post(
            &base(&w),
            &w.admin.token,
            json!({ "userId": newbie.id, "shouldNotify": true }),
        )
        .await;
    assert_eq!(res.status, 201, "{}", res.text());
    let d = data(&res);
    assert_eq!(
        (
            d["role"].clone(),
            d["shouldNotify"].clone(),
            d["projectId"].clone()
        ),
        (json!("agent"), json!(true), json!(w.project.id))
    );
    assert_eq!(d["user"]["email"], "new@example.com");

    let dup = w.post(&base(&w), &w.owner.token, body).await;
    assert_eq!(
        (dup.status.as_u16(), dup.error_code().as_str()),
        (409, "CONFLICT")
    );
    let unknown = w
        .post(
            &base(&w),
            &w.owner.token,
            json!({ "userId": Uuid::new_v4() }),
        )
        .await;
    assert_eq!(unknown.status, 404);
    for bad in [
        json!({}),
        json!({ "userId": "x" }),
        json!({ "userId": newbie.id, "role": "owner" }),
    ] {
        assert_eq!(w.post(&base(&w), &w.owner.token, bad).await.status, 422);
    }
    let admin = visible_user(&w, "adm2@example.com", "side-2").await;
    let r = w
        .post(
            &base(&w),
            &w.owner.token,
            json!({ "userId": admin.id, "role": "admin" }),
        )
        .await;
    assert_eq!(data(&r)["role"], "admin");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn patch_member_flags_and_roles(pool: PgPool) {
    let w = world(pool).await;
    let path = format!("{}/{}", base(&w), w.agent.id);
    let body = Some(json!({ "canManageTemplates": true }));
    assert_eq!(
        w.matrix(Method::PATCH, &path, body).await,
        [200, 200, 403, 404, 404]
    );
    let res = w
        .patch(
            &path,
            &w.admin.token,
            json!({ "role": "admin", "shouldNotify": true, "canViewAllTickets": false }),
        )
        .await;
    let d = data(&res);
    assert_eq!(
        (
            d["role"].clone(),
            d["shouldNotify"].clone(),
            d["canViewAllTickets"].clone(),
            d["canManageTemplates"].clone()
        ),
        (json!("admin"), json!(true), json!(false), json!(true))
    );
    assert_eq!(
        w.patch(&path, &w.owner.token, json!({ "role": "boss" }))
            .await
            .status,
        422
    );
    let missing = format!("{}/{}", base(&w), Uuid::new_v4());
    assert_eq!(
        w.patch(&missing, &w.owner.token, json!({})).await.status,
        404
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn the_owner_cannot_be_demoted_or_removed(pool: PgPool) {
    let w = world(pool).await;
    let owner = format!("{}/{}", base(&w), w.owner.id);
    // Demoting the owner is a conflict for the owner and for another admin; other flags are fine.
    for who in [&w.owner, &w.admin] {
        let r = w
            .patch(&owner, &who.token, json!({ "role": "agent" }))
            .await;
        assert_eq!(
            (r.status.as_u16(), r.error_code().as_str()),
            (409, "CONFLICT")
        );
    }
    assert_eq!(
        w.patch(&owner, &w.admin.token, json!({ "shouldNotify": false }))
            .await
            .status,
        200
    );
    assert_eq!(
        w.patch(&owner, &w.admin.token, json!({ "role": "admin" }))
            .await
            .status,
        200
    );
    // Removal: refused for an admin and for the owner removing themself.
    for who in [&w.admin, &w.owner] {
        let r = w.delete(&owner, &who.token).await;
        assert_eq!(
            (r.status.as_u16(), r.error_code().as_str()),
            (409, "CONFLICT")
        );
    }
    // An agent cannot even try (403 comes first), an outsider sees 404.
    assert_eq!(w.delete(&owner, &w.agent.token).await.status, 403);
    assert_eq!(w.delete(&owner, &w.other.token).await.status, 404);
    let still = data(&w.get(&base(&w), &w.owner.token).await);
    assert_eq!(still.as_array().unwrap().len(), 3);
    // The project still has its owner as admin.
    let role = dp_db::members::role_of(&w.t.pool, w.owner.id, w.project.id)
        .await
        .unwrap();
    assert_eq!(role, Some(Role::Admin));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn remove_member_rules(pool: PgPool) {
    let w = world(pool).await;
    let agent = format!("{}/{}", base(&w), w.agent.id);
    let admin = format!("{}/{}", base(&w), w.admin.id);
    // An agent may not remove somebody else, an outsider sees 404.
    assert_eq!(w.delete(&admin, &w.agent.token).await.status, 403);
    assert_eq!(w.delete(&agent, &w.other.token).await.status, 404);
    // Admin removes the agent; a repeat is 404.
    let r = w.delete(&agent, &w.admin.token).await;
    assert_eq!(
        (r.status.as_u16(), data(&r)),
        (200, json!({ "success": true }))
    );
    assert_eq!(w.delete(&agent, &w.admin.token).await.status, 404);
    assert_eq!(
        w.get(&base(&w), &w.agent.token).await.status,
        404,
        "removed member lost access"
    );
    // A member may remove themself.
    assert_eq!(w.delete(&admin, &w.admin.token).await.status, 200);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn member_routes_do_not_cross_tenants(pool: PgPool) {
    let w = world(pool).await;
    // Project B's member id used under project A's path (and the reverse) is just "not found".
    let foreign_member = format!("{}/{}", base(&w), w.other.id);
    assert_eq!(
        w.patch(&foreign_member, &w.owner.token, json!({ "role": "agent" }))
            .await
            .status,
        404
    );
    assert_eq!(w.delete(&foreign_member, &w.owner.token).await.status, 404);
    // The owner of A has no rights in B at all.
    let b = format!("/projects/{}/members", w.foreign.id);
    assert_eq!(w.get(&b, &w.owner.token).await.status, 404);
    assert_eq!(
        w.post(&b, &w.owner.token, json!({ "userId": w.agent.id }))
            .await
            .status,
        404
    );
    assert_eq!(
        w.patch(
            &format!("{b}/{}", w.other.id),
            &w.owner.token,
            json!({ "shouldNotify": true })
        )
        .await
        .status,
        404
    );
    assert_eq!(
        w.delete(&format!("{b}/{}", w.other.id), &w.owner.token)
            .await
            .status,
        404
    );
    // Being admin of A does not help in B, and B is unchanged.
    assert_eq!(
        data(&w.get(&b, &w.other.token).await)
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        w.get(&format!("{}/search", b), &w.admin.token).await.status,
        404
    );
}
