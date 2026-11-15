mod common;
mod tickets_fixture;

use axum::http::Method;
use dp_db::Role;
use serde_json::json;
use sqlx::PgPool;
use tickets_fixture::*;

const MIG: &str = "../db/migrations";

#[sqlx::test(migrations = "../db/migrations")]
async fn admin_create_happy_path(pool: PgPool) {
    let e = Env::new(pool).await;
    let t = e.ticket(&e.agent, "Printer on fire").await;
    assert_eq!(t["ticketNumber"], 1);
    assert_eq!(t["status"], "pending");
    assert_eq!(t["priority"], "normal");
    assert_eq!(t["client"]["email"], "client@example.com");
    assert_eq!(t["client"]["name"], "client");
    assert_eq!(t["assignedTo"], json!(null));
    assert_eq!(t["project"]["name"], "Project acme");
    // first message is a client message, ticket is unread
    let id = t["id"].as_str().unwrap();
    let m = e.get(&format!("/tickets/{id}/messages"), &e.owner).await;
    assert_eq!(m.json()["data"][0]["senderType"], "client");
    assert_eq!(m.json()["data"][0]["body"], "It is broken");
    let u = e.get("/unread/", &e.owner).await;
    assert_eq!(u.json()["data"][0]["ticketId"], id);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn admin_create_validation_and_roles(pool: PgPool) {
    let e = Env::new(pool).await;
    let pid = e.project.id.to_string();
    let ok = [
        ("projectId", pid.as_str()),
        ("clientEmail", "a@b.co"),
        ("subject", "Valid subject"),
        ("description", "d"),
    ];
    // 401
    assert_eq!(e.form("/tickets/admin", None, None, &ok).await.status, 401);
    // outsider (even global admin): 404, same as unknown project
    let out = e.form("/tickets/admin", Some(&e.outsider), None, &ok).await;
    assert_eq!(out.status, 404);
    let missing_id = uuid::Uuid::new_v4().to_string();
    let missing = [
        ("projectId", missing_id.as_str()),
        ("clientEmail", "a@b.co"),
        ("subject", "Valid subject"),
        ("description", "d"),
    ];
    let miss = e
        .form("/tickets/admin", Some(&e.owner), None, &missing)
        .await;
    assert_eq!(miss.status, 404);
    assert_eq!(out.json(), miss.json());
    // field validation
    let long = "x".repeat(20_001);
    let bad = [
        ("projectId", "nope"),
        ("clientEmail", "not-an-email"),
        ("subject", "abc"),
        ("description", long.as_str()),
        ("priority", "meh"),
    ];
    let res = e.form("/tickets/admin", Some(&e.agent), None, &bad).await;
    assert_eq!(res.status, 422);
    let d = &res.json()["error"]["details"];
    for f in [
        "projectId",
        "clientEmail",
        "subject",
        "description",
        "priority",
    ] {
        assert!(d.get(f).is_some(), "{f} missing in {d}");
    }
    let empty = [
        ("projectId", pid.as_str()),
        ("clientEmail", "a@b.co"),
        ("subject", "Valid subject"),
    ];
    assert!(
        e.form("/tickets/admin", Some(&e.agent), None, &empty)
            .await
            .json()["error"]["details"]["description"]
            .is_string()
    );
    // not multipart
    let res = e.post("/tickets/admin", &e.agent, json!({})).await;
    assert_eq!(res.status, 415);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn parallel_creates_are_unique_and_consecutive(pool: PgPool) {
    let e = std::sync::Arc::new(Env::new(pool).await);
    let mut tasks = Vec::new();
    for i in 0..20 {
        let e = e.clone();
        tasks.push(tokio::spawn(async move {
            let tok = if i % 2 == 0 { &e.owner } else { &e.agent };
            e.ticket(tok, &format!("Parallel ticket {i}")).await["ticketNumber"]
                .as_i64()
                .unwrap()
        }));
    }
    let mut nums = Vec::new();
    for t in tasks {
        nums.push(t.await.unwrap());
    }
    nums.sort();
    assert_eq!(nums, (1..=20).collect::<Vec<_>>());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn get_is_tenant_scoped(pool: PgPool) {
    let e = Env::new(pool).await;
    let id = e.ticket_id(&e.owner, "Tenant ticket").await;
    let path = format!("/tickets/{id}");
    assert_eq!(e.call(Method::GET, &path, None, None).await.status, 401);
    assert_eq!(e.get(&path, &e.owner).await.status, 200);
    assert_eq!(e.get(&path, &e.agent).await.status, 200);
    let out = e.get(&path, &e.outsider).await;
    let missing = e
        .get(&format!("/tickets/{}", uuid::Uuid::new_v4()), &e.outsider)
        .await;
    assert_eq!((out.status.as_u16(), missing.status.as_u16()), (404, 404));
    assert_eq!(out.json(), missing.json());
    assert_eq!(e.get("/tickets/not-a-uuid", &e.owner).await.status, 400);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn list_filters_stats_and_isolation(pool: PgPool) {
    let e = Env::new(pool).await;
    let a = e.ticket_id(&e.owner, "First ticket").await;
    let b = e.ticket_id(&e.owner, "Second ticket").await;
    let _c = e.ticket_id(&e.owner, "Third ticket").await;
    let agent = e.agent_id.to_string();
    e.patch(
        &format!("/tickets/{a}"),
        &e.owner,
        json!({"status":"in_progress","assignedToUserId":agent}),
    )
    .await;
    e.patch(
        &format!("/tickets/{b}"),
        &e.owner,
        json!({"priority":"urgent"}),
    )
    .await;

    let all = e.get("/tickets", &e.agent).await;
    assert_eq!(all.json()["pagination"]["total"], 3);
    assert_eq!(all.json()["stats"]["all"], 3);
    // newest-updated first: b was patched last
    assert_eq!(ids(&all)[0], b);

    let by_status = e.get("/tickets?status=in_progress", &e.agent).await;
    assert_eq!(ids(&by_status), vec![a.clone()]);
    // stats ignore the status filter
    assert_eq!(by_status.json()["stats"]["all"], 3);
    assert_eq!(by_status.json()["stats"]["in_progress"], 1);
    assert_eq!(
        ids(&e
            .get(&format!("/tickets?assignedToUserId={agent}"), &e.agent)
            .await),
        vec![a.clone()]
    );
    assert_eq!(
        ids(&e.get("/tickets?priority=urgent", &e.agent).await),
        vec![b.clone()]
    );
    let pid = e.project.id;
    assert_eq!(
        e.get(&format!("/tickets?projectId={pid}"), &e.agent)
            .await
            .json()["pagination"]["total"],
        3
    );

    // outsider sees nothing, also when naming the project
    let out = e
        .get(&format!("/tickets?projectId={pid}"), &e.outsider)
        .await;
    assert_eq!(out.status, 200);
    assert_eq!(out.json()["data"], json!([]));
    assert_eq!(out.json()["stats"]["all"], 0);

    for q in [
        "status=bogus",
        "priority=x",
        "projectId=x",
        "assignedToUserId=x",
    ] {
        assert_eq!(
            e.get(&format!("/tickets?{q}"), &e.agent).await.status,
            422,
            "{q}"
        );
    }
    assert_eq!(
        e.call(Method::GET, "/tickets", None, None).await.status,
        401
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn unread_first_orders_unread_on_top(pool: PgPool) {
    let e = Env::new(pool).await;
    let a = e.ticket_id(&e.owner, "Older ticket").await;
    let b = e.ticket_id(&e.owner, "Newer ticket").await;
    e.delete(&format!("/unread/ticket/{b}"), &e.owner).await;
    assert_eq!(ids(&e.get("/tickets", &e.owner).await)[0], b);
    assert_eq!(
        ids(&e.get("/tickets?unreadFirst=true", &e.owner).await)[0],
        a
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn pagination_is_bounded(pool: PgPool) {
    let e = Env::new(pool).await;
    for i in 0..5 {
        e.ticket(&e.owner, &format!("Paged ticket {i}")).await;
    }
    let r = e.get("/tickets?limit=2&offset=1", &e.owner).await;
    let p = &r.json()["pagination"];
    assert_eq!(
        (
            p["total"].as_i64(),
            p["limit"].as_i64(),
            p["offset"].as_i64()
        ),
        (Some(5), Some(2), Some(1))
    );
    assert_eq!(p["hasMore"], true);
    assert_eq!(r.json()["data"].as_array().unwrap().len(), 2);
    let last = e.get("/tickets?limit=2&offset=4", &e.owner).await;
    assert_eq!(last.json()["pagination"]["hasMore"], false);
    // over the cap is clamped, junk is 422
    assert_eq!(
        e.get("/tickets?limit=100000", &e.owner).await.json()["pagination"]["limit"],
        200
    );
    for q in ["limit=0", "limit=-3", "limit=abc", "offset=-1"] {
        assert_eq!(
            e.get(&format!("/tickets?{q}"), &e.owner).await.status,
            422,
            "{q}"
        );
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn search_covers_messages_literally_and_is_scoped(pool: PgPool) {
    let e = Env::new(pool).await;
    let a = e.ticket_id(&e.owner, "Unrelated subject").await;
    let _b = e.ticket_id(&e.owner, "Another subject").await;
    e.reply(&a, &e.owner, j("needle in a thread 100% sure"))
        .await;
    e.reply(
        &a,
        &e.owner,
        json!({"body":"secret note about zebra","isInternal":true}),
    )
    .await;

    assert_eq!(
        ids(&e.get("/tickets/search?q=NEEDLE", &e.agent).await),
        vec![a.clone()]
    );
    // members search internal notes too
    assert_eq!(
        ids(&e.get("/tickets/search?q=zebra", &e.agent).await),
        vec![a.clone()]
    );
    // % is literal
    assert_eq!(
        ids(&e.get("/tickets/search?q=100%25", &e.agent).await),
        vec![a.clone()]
    );
    assert!(ids(&e.get("/tickets/search?q=%25%25", &e.agent).await).is_empty());
    let s = e.get("/tickets/search?q=subject", &e.agent).await;
    assert_eq!(s.json()["pagination"]["total"], 2);
    // outsider finds nothing
    assert!(ids(&e.get("/tickets/search?q=needle", &e.outsider).await).is_empty());
    // validation
    assert_eq!(e.get("/tickets/search?q=a", &e.agent).await.status, 422);
    assert_eq!(e.get("/tickets/search", &e.agent).await.status, 422);
    assert_eq!(
        e.get("/tickets/search?q=ab&limit=0", &e.agent).await.status,
        422
    );
    assert_eq!(
        e.call(Method::GET, "/tickets/search?q=ab", None, None)
            .await
            .status,
        401
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn counts_per_project(pool: PgPool) {
    let e = Env::new(pool).await;
    let a = e.ticket_id(&e.owner, "Count one").await;
    e.ticket_id(&e.owner, "Count two").await;
    e.patch(
        &format!("/tickets/{a}"),
        &e.owner,
        json!({"status":"resolved"}),
    )
    .await;
    let r = e.get("/tickets/counts", &e.agent).await;
    let d = &r.json()["data"];
    assert_eq!(d["global"]["all"], 2);
    assert_eq!(d["global"]["resolved"], 1);
    assert_eq!(d["projects"][e.project.id.to_string()]["pending"], 1);
    let out = e.get("/tickets/counts", &e.outsider).await;
    assert_eq!(out.json()["data"]["global"]["all"], 0);
    assert_eq!(out.json()["data"]["projects"], json!({}));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn update_status_machine_and_closed_at(pool: PgPool) {
    let e = Env::new(pool).await;
    let id = e.ticket_id(&e.owner, "Status ticket").await;
    let path = format!("/tickets/{id}");
    let r = e.patch(&path, &e.agent, json!({"status":"resolved"})).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.json()["data"]["closedAt"].is_string());
    // same status: no-op
    assert_eq!(
        e.patch(&path, &e.agent, json!({"status":"resolved"}))
            .await
            .status,
        200
    );
    // reopen clears closedAt
    let r = e
        .patch(&path, &e.agent, json!({"status":"in_progress"}))
        .await;
    assert_eq!(r.json()["data"]["closedAt"], json!(null));
    let r = e.patch(&path, &e.agent, json!({"status":"closed"})).await;
    assert!(r.json()["data"]["closedAt"].is_string());
    // closed is final except pending
    let r = e
        .patch(&path, &e.agent, json!({"status":"in_progress"}))
        .await;
    assert_eq!(
        (r.status.as_u16(), r.error_code().as_str()),
        (409, "CONFLICT")
    );
    assert_eq!(
        e.get(&path, &e.agent).await.json()["data"]["status"],
        "closed"
    );
    let r = e.patch(&path, &e.agent, json!({"status":"pending"})).await;
    assert_eq!(r.json()["data"]["status"], "pending");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn update_validation_and_isolation(pool: PgPool) {
    let e = Env::new(pool).await;
    let id = e.ticket_id(&e.owner, "Patch ticket").await;
    let path = format!("/tickets/{id}");
    for body in [
        json!({"status":"done"}),
        json!({"priority":"x"}),
        json!({"subject":"abc"}),
        json!({"description":""}),
        json!({}),
        json!({"assignedToUserId":"nope"}),
    ] {
        assert_eq!(
            e.patch(&path, &e.agent, body.clone()).await.status,
            422,
            "{body}"
        );
    }
    let ok = json!({"subject":"New subject"});
    assert_eq!(
        e.call(Method::PATCH, &path, None, Some(ok.clone()))
            .await
            .status,
        401
    );
    let out = e.patch(&path, &e.outsider, ok.clone()).await;
    assert_eq!(out.status, 404);
    assert_eq!(
        e.get(&path, &e.agent).await.json()["data"]["subject"],
        "Patch ticket"
    );
    let r = e.patch(&path, &e.agent, ok).await;
    assert_eq!(r.json()["data"]["subject"], "New subject");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn assign_and_unassign(pool: PgPool) {
    let e = Env::new(pool).await;
    let id = e.ticket_id(&e.owner, "Assign ticket").await;
    let path = format!("/tickets/{id}");
    let r = e
        .patch(&path, &e.owner, json!({"assignedToUserId": e.agent_id}))
        .await;
    assert_eq!(r.json()["data"]["assignedTo"]["id"], e.agent_id.to_string());
    // priority-only patch keeps the assignee
    let r = e.patch(&path, &e.owner, json!({"priority":"high"})).await;
    assert_eq!(r.json()["data"]["assignedToUserId"], e.agent_id.to_string());
    // non-member assignee: 422
    let outsider = e.t.user("third@example.com", Role::Agent).await;
    let r = e
        .patch(&path, &e.owner, json!({"assignedToUserId": outsider.id}))
        .await;
    assert_eq!(r.status, 422);
    assert!(r.json()["error"]["details"]["assignedToUserId"].is_string());
    // null unassigns
    let r = e
        .patch(&path, &e.owner, json!({"assignedToUserId": null}))
        .await;
    assert_eq!(r.json()["data"]["assignedToUserId"], json!(null));
    assert_eq!(r.json()["data"]["assignedTo"], json!(null));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn delete_role_matrix(pool: PgPool) {
    let e = Env::new(pool).await;
    let id = e.ticket_id(&e.owner, "Delete me").await;
    let path = format!("/tickets/{id}");
    assert_eq!(e.call(Method::DELETE, &path, None, None).await.status, 401);
    assert_eq!(e.delete(&path, &e.outsider).await.status, 404);
    assert_eq!(e.delete(&path, &e.agent).await.status, 403);
    let r = e.delete(&path, &e.owner).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["data"], json!({"success": true, "id": id}));
    assert_eq!(e.get(&path, &e.owner).await.status, 404);
    assert_eq!(e.delete(&path, &e.owner).await.status, 404);
    let _ = MIG;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn members_without_view_all_only_see_their_tickets(pool: PgPool) {
    let e = Env::new(pool).await;
    let mine = e.ticket_id(&e.owner, "Assigned to agent").await;
    let other = e.ticket_id(&e.owner, "Not for the agent").await;
    e.patch(
        &format!("/tickets/{mine}"),
        &e.owner,
        json!({"assignedToUserId": e.agent_id}),
    )
    .await;
    sqlx::query("UPDATE project_members SET can_view_all_tickets = false WHERE user_id = $1")
        .bind(e.agent_id)
        .execute(&e.t.pool)
        .await
        .unwrap();
    assert_eq!(ids(&e.get("/tickets", &e.agent).await), vec![mine.clone()]);
    assert_eq!(
        e.get(&format!("/tickets/{other}"), &e.agent).await.status,
        404
    );
    assert_eq!(
        e.get(&format!("/tickets/{other}/messages"), &e.agent)
            .await
            .status,
        404
    );
    assert_eq!(e.reply(&other, &e.agent, j("hi")).await.status, 404);
}
