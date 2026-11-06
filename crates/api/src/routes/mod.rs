pub mod attachments;
pub mod auth;
pub mod health;
pub mod incoming;
pub mod tickets;

// R21: projects, members, users, clients, templates, email responses, project mail settings.
pub mod clients;
pub mod email_responses;
pub mod mail_guard;
pub mod members;
pub mod project_mail;
pub mod projects;
pub mod qparams;
pub mod templates;
pub mod users;

/// Routers of the project/team area, mounted under `/api/{version}` by `build_app_with`.
pub fn project_routes() -> axum::Router<crate::AppState> {
    axum::Router::new()
        .nest(
            "/projects",
            projects::router()
                .merge(members::router())
                .merge(project_mail::router()),
        )
        .merge(users::router())
        .merge(clients::router())
        .merge(templates::router())
        .merge(email_responses::router())
}
