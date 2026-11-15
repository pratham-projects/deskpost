mod common;
mod r21_world;

use axum::http::Method;
use dp_db::Role;
use r21_world::*;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrations = "../db/migrations")]
async fn me_and_profile_update(pool: PgPool) {
    let w = world(pool).await;
    let res = w.get("/users/me", &w.agent.token).await;
    assert_eq!(data(&res)["email"], "agent@example.com");
    assert_no_secrets(&res, &[PASSWORD]);
    assert_eq!(
        w.call(Method::GET, "/users/me", None, None)
            .await
            .error_code(),
        "TOKEN_MISSING"
    );

    let res = w
        .patch("/users/me", &w.agent.token, json!({ "name": " New Name ", "phone": "+1 555", "timezone": "Europe/Berlin", "role": "admin", "email": "x@y.zz" }))
        .await;
    let d = data(&res);
    assert_eq!(
        (
            d["name"].clone(),
            d["phone"].clone(),
            d["timezone"].clone(),
            d["role"].clone(),
            d["email"].clone()
        ),
        (
            json!("New Name"),
            json!("+1 555"),
            json!("Europe/Berlin"),
            json!("agent"),
            json!("agent@example.com")
        ),
        "role and email are not editable here"
    );
    let cleared = w
        .patch("/users/me", &w.agent.token, json!({ "phone": "" }))
        .await;
    assert!(data(&cleared)["phone"].is_null());
    for bad in [
        json!({ "name": "" }),
        json!({ "name": "x".repeat(256) }),
        json!({ "phone": "1".repeat(21) }),
        json!({ "timezone": "" }),
    ] {
        let r = w.patch("/users/me", &w.agent.token, bad).await;
        assert_eq!(r.status, 422);
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn password_change_needs_the_current_password(pool: PgPool) {
    let w = world(pool).await;
    let path = "/users/me/password";
    let ok = json!({ "currentPassword": PASSWORD, "newPassword": "brand-new-pass-9" });

    let wrong = w
        .post(
            path,
            &w.agent.token,
            json!({ "currentPassword": "nope-nope-1", "newPassword": "brand-new-pass-9" }),
        )
        .await;
    assert_eq!(wrong.status, 422);
    assert!(
        wrong.json()["error"]["details"]
            .get("currentPassword")
            .is_some()
    );
    for bad in [
        json!({}),
        json!({ "currentPassword": PASSWORD, "newPassword": "short" }),
        json!({ "currentPassword": PASSWORD, "newPassword": "x".repeat(129) }),
    ] {
        assert_eq!(w.post(path, &w.agent.token, bad).await.status, 422);
    }
    assert_eq!(
        w.call(Method::POST, path, None, Some(ok.clone()))
            .await
            .status,
        401
    );
    // Nothing changed so far: the old password still signs in.
    let session = w.t.signin("agent@example.com").await;

    let res = w.post(path, &w.agent.token, ok).await;
    assert_eq!(res.status, 200, "{}", res.text());
    assert_no_secrets(&res, &["brand-new-pass-9", PASSWORD]);
    let old =
        w.t.send_json(
            w.t.post("/auth/signin"),
            json!({ "email": "agent@example.com", "password": PASSWORD }),
        )
        .await;
    assert_eq!(old.status, 401);
    let new =
        w.t.send_json(
            w.t.post("/auth/signin"),
            json!({ "email": "agent@example.com", "password": "brand-new-pass-9" }),
        )
        .await;
    assert_eq!(new.status, 200);
    // Existing sessions were signed out.
    let refresh =
        w.t.send(w.t.post("/auth/refresh").cookie(&session.cookie_pair()))
            .await;
    assert_eq!(refresh.status, 401);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn password_guess_attempts_are_rate_limited(pool: PgPool) {
    let w = world(pool).await;
    let guess = json!({ "currentPassword": "wrong-wrong-1", "newPassword": "brand-new-pass-9" });
    let mut last = 0;
    for _ in 0..12 {
        last = w
            .post("/users/me/password", &w.agent.token, guess.clone())
            .await
            .status
            .as_u16();
    }
    assert_eq!(last, 429);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn user_list_is_scoped_to_shared_projects(pool: PgPool) {
    let w = world(pool).await;
    let all = w.get("/users", &w.root.token).await;
    assert_eq!(
        all.json()["pagination"]["total"],
        5,
        "a global admin sees everyone"
    );
    assert_no_secrets(&all, &[PASSWORD]);

    let mine = w.get("/users", &w.agent.token).await;
    let mut emails: Vec<_> = data(&mine)
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["email"].as_str().unwrap().to_string())
        .collect();
    emails.sort();
    assert_eq!(
        emails,
        [
            "admin@example.com",
            "agent@example.com",
            "owner@example.com"
        ]
    );
    // The other tenant's owner sees only themself.
    assert_eq!(
        data(&w.get("/users", &w.other.token).await)
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let admins = w.get("/users?role=admin", &w.root.token).await;
    assert_eq!(data(&admins).as_array().unwrap().len(), 1);
    assert_eq!(
        data(&w.get("/users?status=suspended", &w.root.token).await),
        json!([])
    );
    assert_eq!(
        w.get("/users?limit=2&offset=1", &w.root.token).await.json()["pagination"]["hasMore"],
        true
    );
    for q in ["role=boss", "status=x", "limit=0", "offset=-3"] {
        assert_eq!(
            w.get(&format!("/users?{q}"), &w.root.token).await.status,
            422,
            "{q}"
        );
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn get_user_is_visible_to_self_comembers_and_global_admins(pool: PgPool) {
    let w = world(pool).await;
    let of = |a: &Actor| format!("/users/{}", a.id);
    assert_eq!(w.get(&of(&w.agent), &w.agent.token).await.status, 200);
    assert_eq!(
        w.get(&of(&w.owner), &w.agent.token).await.status,
        200,
        "co-member"
    );
    assert_eq!(w.get(&of(&w.agent), &w.root.token).await.status, 200);
    let hidden = w.get(&of(&w.other), &w.agent.token).await;
    let missing = w
        .get(&format!("/users/{}", Uuid::new_v4()), &w.agent.token)
        .await;
    assert_eq!(hidden.status, 404);
    assert_eq!(hidden.json(), missing.json());
    assert_no_secrets(&w.get(&of(&w.owner), &w.agent.token).await, &[PASSWORD]);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn only_global_admins_manage_accounts(pool: PgPool) {
    let w = world(pool).await;
    let create = json!({ "email": "New@Example.com", "password": "long-enough-1", "name": "New", "role": "admin" });
    for who in [&w.owner, &w.agent, &w.other] {
        assert_eq!(
            w.post("/users", &who.token, create.clone()).await.status,
            403
        );
        assert_eq!(
            w.patch(
                &format!("/users/{}", w.agent.id),
                &who.token,
                json!({ "name": "x" })
            )
            .await
            .status,
            403
        );
        assert_eq!(
            w.delete(&format!("/users/{}", w.agent.id), &who.token)
                .await
                .status,
            403
        );
    }
    let res = w.post("/users", &w.root.token, create.clone()).await;
    assert_eq!(res.status, 201, "{}", res.text());
    assert_no_secrets(&res, &["long-enough-1"]);
    assert_eq!(
        (data(&res)["email"].clone(), data(&res)["role"].clone()),
        (json!("new@example.com"), json!("admin"))
    );
    // The new account can sign in with that password.
    let si =
        w.t.send_json(
            w.t.post("/auth/signin"),
            json!({ "email": "new@example.com", "password": "long-enough-1" }),
        )
        .await;
    assert_eq!(si.status, 200);

    let dup = w
        .post(
            "/users",
            &w.root.token,
            json!({ "email": "NEW@example.com", "password": "long-enough-1", "name": "N" }),
        )
        .await;
    assert_eq!(
        (dup.status.as_u16(), dup.error_code().as_str()),
        (409, "CONFLICT")
    );
    for bad in [
        json!({ "email": "nope", "password": "long-enough-1", "name": "N" }),
        json!({ "email": "a@b.co", "password": "short", "name": "N" }),
        json!({ "email": "a@b.co", "password": "long-enough-1", "name": "" }),
        json!({ "email": "a@b.co", "password": "long-enough-1", "name": "N", "role": "owner" }),
    ] {
        assert_eq!(w.post("/users", &w.root.token, bad).await.status, 422);
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn admin_patch_and_the_last_admin_rule(pool: PgPool) {
    let w = world(pool).await;
    let agent = format!("/users/{}", w.agent.id);
    let res = w
        .patch(
            &agent,
            &w.root.token,
            json!({ "name": "Renamed", "status": "suspended", "role": "admin" }),
        )
        .await;
    let d = data(&res);
    assert_eq!(
        (d["name"].clone(), d["status"].clone(), d["role"].clone()),
        (json!("Renamed"), json!("suspended"), json!("admin"))
    );
    // A suspended user's live token stops working.
    assert_eq!(w.get("/users/me", &w.agent.token).await.status, 401);
    for bad in [
        json!({ "role": "owner" }),
        json!({ "status": "gone" }),
        json!({ "name": "" }),
    ] {
        assert_eq!(w.patch(&agent, &w.root.token, bad).await.status, 422);
    }
    assert_eq!(
        w.patch(
            &format!("/users/{}", Uuid::new_v4()),
            &w.root.token,
            json!({ "name": "x" })
        )
        .await
        .status,
        404
    );

    // Now two active admins exist (root and agent is suspended, so root is the last one).
    let root = format!("/users/{}", w.root.id);
    for body in [json!({ "role": "agent" }), json!({ "status": "inactive" })] {
        let r = w.patch(&root, &w.root.token, body).await;
        assert_eq!(
            (r.status.as_u16(), r.error_code().as_str()),
            (409, "CONFLICT")
        );
    }
    let r = w.delete(&root, &w.root.token).await;
    assert_eq!(r.status, 409);
    // With a second active admin the first may step down.
    assert_eq!(
        w.patch(&agent, &w.root.token, json!({ "status": "active" }))
            .await
            .status,
        200
    );
    assert_eq!(
        w.patch(&root, &w.root.token, json!({ "role": "agent" }))
            .await
            .status,
        200
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn delete_user_rules(pool: PgPool) {
    let w = world(pool).await;
    // Owners cannot be deleted while they own a project.
    let r = w
        .delete(&format!("/users/{}", w.owner.id), &w.root.token)
        .await;
    assert_eq!(
        (r.status.as_u16(), r.error_code().as_str()),
        (409, "CONFLICT")
    );
    let r = w
        .delete(&format!("/users/{}", w.agent.id), &w.root.token)
        .await;
    assert_eq!(
        (r.status.as_u16(), data(&r)),
        (200, json!({ "success": true, "id": w.agent.id }))
    );
    assert_eq!(
        w.delete(&format!("/users/{}", w.agent.id), &w.root.token)
            .await
            .status,
        404
    );
    let members = data(
        &w.get(
            &format!("/projects/{}/members", w.project.id),
            &w.owner.token,
        )
        .await,
    );
    assert_eq!(
        members.as_array().unwrap().len(),
        2,
        "memberships went with the user"
    );
    let _ = Role::Agent;
}
