//! Fixture for the ticket route tests: owner (project admin), agent member and an outsider who is
//! a *global* admin, a project with a known widget key, and a recording mailer.
#![allow(dead_code)]

use std::sync::Arc;

use api::mail_out::{Mailer, RecordingMailer};
use api::{AppState, build_app_with};
use axum::Router;
use axum::body::Body;
use axum::http::{Method, header};
use dp_core::api_key::hash_api_key;
use dp_db::projects::Project;
use dp_db::{Id, Pool, Role};
use serde_json::{Value, json};

use crate::common::*;

pub const KEY: &str = "sdk_test_widget_key";

pub struct Env {
    pub t: TestApp,
    pub mailer: Arc<RecordingMailer>,
    pub project: Project,
    pub owner: String,
    pub agent: String,
    pub outsider: String,
    pub owner_id: Id,
    pub agent_id: Id,
}

pub fn app_with_mailer(pool: Pool, mailer: Arc<RecordingMailer>) -> TestApp {
    let mut state = AppState::new(pool.clone(), test_config()).unwrap();
    state.mailer = Mailer::Recording(mailer);
    let app = build_app_with(state.clone(), Router::new());
    TestApp { app, state, pool }
}

impl Env {
    pub async fn new(pool: Pool) -> Self {
        Self::with_mailer(pool, Arc::new(RecordingMailer::default())).await
    }

    pub async fn with_mailer(pool: Pool, mailer: Arc<RecordingMailer>) -> Self {
        let t = app_with_mailer(pool, mailer.clone());
        let owner = t.user("owner@example.com", Role::Agent).await;
        let agent = t.user("agent@example.com", Role::Agent).await;
        t.user("outsider@example.com", Role::Admin).await;
        let project = t.project(owner.id, "acme").await;
        t.add_member(owner.id, project.id, agent.id, Role::Agent)
            .await;
        sqlx::query("UPDATE projects SET api_key_hash = $2 WHERE id = $1")
            .bind(project.id)
            .bind(hash_api_key(KEY))
            .execute(&t.pool)
            .await
            .unwrap();
        Self {
            owner: t.signin("owner@example.com").await.access_token,
            agent: t.signin("agent@example.com").await.access_token,
            outsider: t.signin("outsider@example.com").await.access_token,
            owner_id: owner.id,
            agent_id: agent.id,
            mailer,
            project,
            t,
        }
    }

    /// JSON request with an optional bearer token.
    pub async fn call(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Resp {
        let mut req = self.t.req(method, path);
        if let Some(tok) = token {
            req = req.bearer(tok);
        }
        match body {
            Some(b) => self.t.send_json(req, b).await,
            None => self.t.send(req).await,
        }
    }

    pub async fn get(&self, path: &str, token: &str) -> Resp {
        self.call(Method::GET, path, Some(token), None).await
    }

    pub async fn post(&self, path: &str, token: &str, body: Value) -> Resp {
        self.call(Method::POST, path, Some(token), Some(body)).await
    }

    pub async fn patch(&self, path: &str, token: &str, body: Value) -> Resp {
        self.call(Method::PATCH, path, Some(token), Some(body))
            .await
    }

    pub async fn delete(&self, path: &str, token: &str) -> Resp {
        self.call(Method::DELETE, path, Some(token), None).await
    }

    /// multipart POST; `token` is the bearer, `key` the widget key header.
    pub async fn form(
        &self,
        path: &str,
        token: Option<&str>,
        key: Option<&str>,
        fields: &[(&str, &str)],
    ) -> Resp {
        let (ct, body) = multipart(fields);
        let mut req = self.t.post(path).header(header::CONTENT_TYPE, &ct);
        if let Some(tok) = token {
            req = req.bearer(tok);
        }
        if let Some(k) = key {
            req = req.header("x-project-key", k);
        }
        self.t.send_body(req, Body::from(body)).await
    }

    pub fn fields<'a>(&'a self, subject: &'a str) -> Vec<(&'a str, String)> {
        vec![
            ("projectId", self.project.id.to_string()),
            ("clientEmail", "Client@Example.com".into()),
            ("subject", subject.into()),
            ("description", "It is broken".into()),
        ]
    }

    /// Creates a ticket through `POST /tickets/admin`; returns the ticket JSON.
    pub async fn ticket(&self, token: &str, subject: &str) -> Value {
        let f = self.fields(subject);
        let f: Vec<(&str, &str)> = f.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let res = self.form("/tickets/admin", Some(token), None, &f).await;
        assert_eq!(res.status, 201, "{}", res.text());
        // Creation also queues the team notice (the owner has shouldNotify); tests here are about
        // replies and confirmations, so start from an empty queue of those.
        sqlx::query("DELETE FROM email_logs WHERE id IN (SELECT email_log_id FROM mail_outbox WHERE kind = 'new_ticket')")
            .execute(&self.t.pool)
            .await
            .unwrap();
        res.json()["data"].clone()
    }

    pub async fn ticket_id(&self, token: &str, subject: &str) -> String {
        self.ticket(token, subject).await["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    pub async fn reply(&self, ticket: &str, token: &str, body: Value) -> Resp {
        self.post(&format!("/tickets/{ticket}/messages"), token, body)
            .await
    }
}

pub fn multipart(fields: &[(&str, &str)]) -> (String, Vec<u8>) {
    let boundary = "XBOUNDARYX";
    let mut out = String::new();
    for (k, v) in fields {
        out.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n"
        ));
    }
    out.push_str(&format!("--{boundary}--\r\n"));
    (
        format!("multipart/form-data; boundary={boundary}"),
        out.into_bytes(),
    )
}

pub fn ids(res: &Resp) -> Vec<String> {
    res.json()["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_string())
        .collect()
}

pub fn j(v: &str) -> Value {
    json!({ "body": v })
}
