//! Fixture for the project/team route tests: one tenant with every role plus a second tenant
//! and a global admin who belongs to neither.
#![allow(dead_code)]

use api::config::{Argon2Cost, Config};
use axum::http::Method;
use dp_db::projects::Project;
use dp_db::{Id, Role};
use serde_json::Value;
use sqlx::PgPool;

pub use crate::common::*;

pub struct W {
    pub t: TestApp,
    /// Tenant A.
    pub project: Project,
    /// Tenant B (owned by `other`).
    pub foreign: Project,
    pub owner: Actor,
    pub admin: Actor,
    pub agent: Actor,
    /// Owns tenant B only.
    pub other: Actor,
    /// Global admin, member of neither project.
    pub root: Actor,
}

pub struct Actor {
    pub id: Id,
    pub email: String,
    pub token: String,
}

pub fn config_allowing_private_hosts() -> Config {
    let mut c = Config::from_lookup(env_with(&[("DP_ALLOW_PRIVATE_MAIL_HOSTS", "1")])).unwrap();
    c.argon2 = Argon2Cost {
        memory_kib: 8,
        iterations: 1,
        parallelism: 1,
    };
    c
}

pub async fn world(pool: PgPool) -> W {
    world_with(TestApp::new(pool).await).await
}

pub async fn world_with(t: TestApp) -> W {
    async fn actor(t: &TestApp, email: &str, role: Role) -> Actor {
        let u = t.user(email, role).await;
        Actor {
            id: u.id,
            email: email.into(),
            token: t.signin(email).await.access_token,
        }
    }
    let root = actor(&t, "root@example.com", Role::Admin).await;
    let owner = actor(&t, "owner@example.com", Role::Agent).await;
    let admin = actor(&t, "admin@example.com", Role::Agent).await;
    let agent = actor(&t, "agent@example.com", Role::Agent).await;
    let other = actor(&t, "other@example.com", Role::Agent).await;
    let project = t.project(owner.id, "acme").await;
    t.add_member(owner.id, project.id, admin.id, Role::Admin)
        .await;
    t.add_member(owner.id, project.id, agent.id, Role::Agent)
        .await;
    let foreign = t.project(other.id, "globex").await;
    W {
        t,
        project,
        foreign,
        owner,
        admin,
        agent,
        other,
        root,
    }
}

impl W {
    pub async fn call(
        &self,
        m: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Resp {
        let mut req = self.t.req(m, path);
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

    /// Status codes of one request sent by owner, admin, agent, other tenant and root, in order.
    pub async fn matrix(&self, m: Method, path: &str, body: Option<Value>) -> [u16; 5] {
        let mut out = [0; 5];
        for (i, a) in [
            &self.owner,
            &self.admin,
            &self.agent,
            &self.other,
            &self.root,
        ]
        .iter()
        .enumerate()
        {
            out[i] = self
                .call(m.clone(), path, Some(&a.token), body.clone())
                .await
                .status
                .as_u16();
        }
        out
    }
}

/// Keys that must never appear in a response body.
const FORBIDDEN_KEYS: [&str; 9] = [
    "password",
    "passwordHash",
    "password_hash",
    "smtpPassword",
    "imapPassword",
    "smtpPasswordEncrypted",
    "imapPasswordEncrypted",
    "apiKeyHash",
    "api_key_hash",
];

fn scan(v: &Value, path: &str, found: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                if FORBIDDEN_KEYS.contains(&k.as_str()) {
                    found.push(format!("{path}.{k}"));
                }
                scan(v, &format!("{path}.{k}"), found);
            }
        }
        Value::Array(a) => a.iter().for_each(|v| scan(v, path, found)),
        _ => {}
    }
}

/// Asserts the body has no secret-bearing key, no ciphertext (`v1:`), no argon2 hash, and none of
/// the plaintext `secrets`.
pub fn assert_no_secrets(res: &Resp, secrets: &[&str]) {
    let text = res.text();
    if let Ok(j) = serde_json::from_str::<Value>(&text) {
        let mut found = Vec::new();
        scan(&j, "$", &mut found);
        assert!(
            found.is_empty(),
            "secret keys in response: {found:?}\n{text}"
        );
    }
    for s in secrets {
        assert!(!text.contains(s), "plaintext secret {s:?} leaked: {text}");
    }
    assert!(!text.contains("\"v1:"), "ciphertext leaked: {text}");
    assert!(!text.contains("$argon2"), "password hash leaked: {text}");
}

pub fn data(res: &Resp) -> Value {
    res.json()["data"].clone()
}
