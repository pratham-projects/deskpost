#![allow(dead_code)]

use sd_db::projects::{NewProject, Project};
use sd_db::tickets::{Access, NewTicket, Ticket};
use sd_db::users::User;
use sd_db::{Id, Pool, Priority, Role, TicketSource, members, projects, tickets, users};

pub async fn user(pool: &Pool, email: &str) -> User {
    users::create(pool, email, &format!("User {email}"), "hash", Role::Agent)
        .await
        .unwrap()
}

pub async fn project(pool: &Pool, owner: Id, slug: &str) -> Project {
    projects::create(
        pool,
        owner,
        &NewProject {
            name: format!("Project {slug}"),
            slug: slug.to_string(),
            domain: None,
            description: None,
            api_key_hash: format!("hash-{slug}"),
            api_key_prefix: "pk_test".into(),
        },
    )
    .await
    .unwrap()
}

pub async fn add_member(pool: &Pool, admin: Id, project: Id, user: Id, role: Role) {
    members::add(pool, admin, project, user, role, false)
        .await
        .unwrap()
        .expect("admin can add members");
}

pub fn new_ticket(project_id: Id, client_email: &str) -> NewTicket {
    NewTicket {
        project_id,
        client_email: client_email.to_string(),
        client_name: Some("Client".into()),
        subject: "Printer on fire".into(),
        description: Some("It is burning".into()),
        priority: Priority::Normal,
        source: TicketSource::Web,
        created_by: None,
        first_message: "It is burning".into(),
    }
}

pub async fn ticket(pool: &Pool, actor: Id, project: Id, client_email: &str) -> Ticket {
    tickets::create(
        pool,
        &Access::Member(actor),
        &new_ticket(project, client_email),
    )
    .await
    .unwrap()
    .expect("actor is a member")
}

/// Two tenants: (owner a, project a, owner b, project b).
pub async fn two_tenants(pool: &Pool) -> (User, Project, User, Project) {
    let a = user(pool, "a@example.com").await;
    let pa = project(pool, a.id, "tenant-a").await;
    let b = user(pool, "b@example.com").await;
    let pb = project(pool, b.id, "tenant-b").await;
    (a, pa, b, pb)
}

pub async fn count(pool: &Pool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .unwrap()
}
