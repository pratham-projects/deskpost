//! R50 regression tests: resolved auto-response, ticket delete cleanup, restricted client list,
//! guarded status update.
mod common;
mod mailkit;

use axum::http::Method;
use common::*;
use dp_db::tickets::{Guarded, TicketPatch};
use dp_db::{EmailResponseStatus, Role, TicketStatus, email_responses};
use mailkit::*;
use serde_json::json;
use sqlx::PgPool;

async fn patch(k: &Kit, tok: &str, id: dp_db::Id, body: serde_json::Value) -> Resp {
    k.t.send_json(
        k.t.req(Method::PATCH, &format!("/tickets/{id}"))
            .bearer(tok),
        body,
    )
    .await
}

#[sqlx::test(migrations = "../db/migrations")]
async fn resolving_queues_the_resolved_auto_response_once(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (owner, project, ticket, s) = k.tenant("owner@example.com", "acme").await;
    email_responses::create(
        &k.t.pool,
        owner.id,
        project.id,
        EmailResponseStatus::Resolved,
        "Done",
        "Resolved.",
        true,
    )
    .await
    .unwrap()
    .unwrap();
    let r = patch(&k, &s.access_token, ticket.id, json!({"status":"resolved"})).await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(k.count("mail_outbox").await, 1);
    // Re-sending the same status and moving on do not queue again.
    patch(&k, &s.access_token, ticket.id, json!({"status":"resolved"})).await;
    patch(&k, &s.access_token, ticket.id, json!({"priority":"high"})).await;
    assert_eq!(k.count("mail_outbox").await, 1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn deleting_a_ticket_removes_its_files(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (_, _, ticket, s) = k.tenant("owner@example.com", "acme").await;
    let req =
        k.t.req(Method::PUT, &format!("/attachments/{}", ticket.id))
            .bearer(&s.access_token)
            .header("content-type", "text/plain")
            .header("x-filename", "a.txt");
    let up =
        k.t.send_body(req, axum::body::Body::from(b"abc".to_vec()))
            .await;
    assert_eq!(up.status, 201, "{}", up.text());
    assert_eq!(k.stored_files(), 2, "data + sidecar");
    let del =
        k.t.send(
            k.t.req(Method::DELETE, &format!("/tickets/{}", ticket.id))
                .bearer(&s.access_token),
        )
        .await;
    assert_eq!(del.status, 200, "{}", del.text());
    assert_eq!((k.count("attachments").await, k.stored_files()), (0, 0));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn restricted_member_lists_only_clients_of_visible_tickets(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (owner, project, mine, _) = k.tenant("owner@example.com", "acme").await;
    let agent = k.t.user("agent@example.com", Role::Agent).await;
    k.t.add_member(owner.id, project.id, agent.id, Role::Agent)
        .await;
    k.ticket(&owner, &project, "other@example.com").await;
    dp_db::tickets::update(
        &k.t.pool,
        owner.id,
        mine.id,
        &TicketPatch {
            assignee: Some(Some(agent.id)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let tok = k.t.signin("agent@example.com").await.access_token;
    let q = format!("/clients?projectId={}", project.id);
    let list = |tok: String| {
        let (k, q) = (&k, q.clone());
        async move { k.t.send(k.t.get(&q).bearer(&tok)).await.json()["data"].clone() }
    };
    assert_eq!(list(tok.clone()).await.as_array().unwrap().len(), 2);
    sqlx::query("UPDATE project_members SET can_view_all_tickets = false WHERE user_id = $1")
        .bind(agent.id)
        .execute(&k.t.pool)
        .await
        .unwrap();
    let rows = list(tok).await;
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["email"], "client@example.com");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn guarded_update_refuses_a_stale_status(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (owner, _, ticket, _) = k.tenant("owner@example.com", "acme").await;
    let closed = TicketPatch {
        status: Some(TicketStatus::Closed),
        ..Default::default()
    };
    dp_db::tickets::update(&k.t.pool, owner.id, ticket.id, &closed)
        .await
        .unwrap();
    // The caller validated `in_progress` against `open`, but the row is already closed.
    let stale = TicketPatch {
        status: Some(TicketStatus::InProgress),
        ..Default::default()
    };
    let r = dp_db::tickets::update_guarded(
        &k.t.pool,
        owner.id,
        ticket.id,
        &stale,
        Some(TicketStatus::Pending),
    )
    .await
    .unwrap();
    assert!(matches!(r, Guarded::Stale(TicketStatus::Closed)), "{r:?}");
    let now = dp_db::tickets::get(&k.t.pool, owner.id, ticket.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(now.status, TicketStatus::Closed);
}

fn form(id: dp_db::Id, extra: &[(&str, &str)]) -> (String, Vec<u8>) {
    let mut fields = vec![
        ("projectId", id.to_string()),
        ("clientEmail", "new@example.com".into()),
        ("subject", "Printer broke".into()),
        ("description", "It is stuck".into()),
    ];
    fields.extend(extra.iter().map(|(k, v)| (*k, v.to_string())));
    let mut body = String::new();
    for (k, v) in &fields {
        body += &format!("--B\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n");
    }
    body += "--B--\r\n";
    ("multipart/form-data; boundary=B".into(), body.into_bytes())
}

#[sqlx::test(migrations = "../db/migrations")]
async fn created_auto_response_is_queued_for_admin_tickets_unless_opted_out(pool: PgPool) {
    let k = Kit::new(pool).await;
    let (owner, project, _, s) = k.tenant("owner@example.com", "acme").await;
    email_responses::create(
        &k.t.pool,
        owner.id,
        project.id,
        EmailResponseStatus::Created,
        "Got it",
        "We got it.",
        true,
    )
    .await
    .unwrap()
    .unwrap();
    let post = |extra: &'static [(&'static str, &'static str)]| {
        let (k, tok, pid) = (&k, s.access_token.clone(), project.id);
        async move {
            let (ct, body) = form(pid, extra);
            let req =
                k.t.req(Method::POST, "/tickets/admin")
                    .bearer(&tok)
                    .header("content-type", &ct);
            k.t.send_body(req, axum::body::Body::from(body)).await
        }
    };
    // The owner's shouldNotify notice is a separate kind; only confirmations are counted here.
    let confirmations = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM mail_outbox WHERE kind = 'created'")
            .fetch_one(&k.t.pool)
            .await
            .unwrap()
    };
    assert_eq!(post(&[]).await.status, 201);
    assert_eq!(confirmations().await, 1);
    assert_eq!(post(&[("sendNotification", "false")]).await.status, 201);
    assert_eq!(confirmations().await, 1);
    assert_eq!(k.count("mail_outbox").await, 3, "one notice per ticket");
}
