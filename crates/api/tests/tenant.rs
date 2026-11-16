//! The reusable access fixture: how a project-scoped route proves tenant isolation.
//! Copy `isolated_project_route` for a new route: owner (admin), agent member, outsider.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use dp_db::Role;
use sqlx::PgPool;
use uuid::Uuid;

struct World {
    t: TestApp,
    project: dp_db::projects::Project,
    owner: String,
    agent: String,
    outsider: String,
}

async fn world(pool: PgPool) -> World {
    let t = TestApp::new(pool).await;
    let owner = t.user("owner@example.com", Role::Agent).await;
    let agent = t.user("agent@example.com", Role::Agent).await;
    t.user("outsider@example.com", Role::Admin).await; // a *global* admin is still an outsider
    let project = t.project(owner.id, "acme").await;
    t.add_member(owner.id, project.id, agent.id, Role::Agent)
        .await;
    World {
        owner: t.signin("owner@example.com").await.access_token,
        agent: t.signin("agent@example.com").await.access_token,
        outsider: t.signin("outsider@example.com").await.access_token,
        project,
        t,
    }
}

async fn call(w: &World, method: Method, id: impl std::fmt::Display, token: Option<&str>) -> Resp {
    let mut req = w.t.req(method, &format!("/_fixture/projects/{id}"));
    if let Some(token) = token {
        req = req.bearer(token);
    }
    w.t.send(req).await
}

#[sqlx::test(migrations = "../db/migrations")]
async fn isolated_project_route(pool: PgPool) {
    let w = world(pool).await;
    let id = w.project.id;

    // Members see it with their project role.
    let res = call(&w, Method::GET, id, Some(&w.owner)).await;
    assert_eq!((res.status, res.text().as_str()), (StatusCode::OK, "admin"));
    let res = call(&w, Method::GET, id, Some(&w.agent)).await;
    assert_eq!((res.status, res.text().as_str()), (StatusCode::OK, "agent"));

    // A non-member gets 404, indistinguishable from a project that does not exist.
    let outsider = call(&w, Method::GET, id, Some(&w.outsider)).await;
    let missing = call(&w, Method::GET, Uuid::new_v4(), Some(&w.owner)).await;
    assert_eq!(outsider.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(outsider.json(), missing.json());
    assert_eq!(outsider.error_code(), "NOT_FOUND");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn admin_only_route_separates_403_from_404(pool: PgPool) {
    let w = world(pool).await;
    let id = w.project.id;
    assert_eq!(
        call(&w, Method::DELETE, id, Some(&w.owner)).await.status,
        StatusCode::OK
    );
    let agent = call(&w, Method::DELETE, id, Some(&w.agent)).await;
    assert_eq!(agent.status, StatusCode::FORBIDDEN);
    assert_eq!(agent.error_code(), "FORBIDDEN");
    assert_eq!(
        call(&w, Method::DELETE, id, Some(&w.outsider)).await.status,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn fixture_route_auth_failures(pool: PgPool) {
    let w = world(pool).await;
    let id = w.project.id;
    let res = call(&w, Method::GET, id, None).await;
    assert_eq!(
        (res.status, res.error_code().as_str()),
        (StatusCode::UNAUTHORIZED, "TOKEN_MISSING")
    );
    let res = call(&w, Method::GET, "not-a-uuid", Some(&w.owner)).await;
    assert_eq!(
        (res.status, res.error_code().as_str()),
        (StatusCode::BAD_REQUEST, "BAD_REQUEST")
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn global_admin_extractor(pool: PgPool) {
    let w = world(pool).await;
    let get = |token: &str| w.t.send(w.t.get("/_fixture/global-admin").bearer(token));
    assert_eq!(get(&w.outsider).await.status, StatusCode::OK);
    let res = get(&w.agent).await;
    assert_eq!(
        (res.status, res.error_code().as_str()),
        (StatusCode::FORBIDDEN, "FORBIDDEN")
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn deactivated_user_loses_access_with_a_live_token(pool: PgPool) {
    let w = world(pool).await;
    let ok = w.t.send(w.t.get("/_fixture/whoami").bearer(&w.agent)).await;
    assert_eq!(ok.text(), "agent@example.com");
    sqlx::query("UPDATE users SET status = 'inactive' WHERE email = 'agent@example.com'")
        .execute(&w.t.pool)
        .await
        .unwrap();
    let res = w.t.send(w.t.get("/_fixture/whoami").bearer(&w.agent)).await;
    assert_eq!(
        (res.status, res.error_code().as_str()),
        (StatusCode::UNAUTHORIZED, "TOKEN_INVALID")
    );
}
