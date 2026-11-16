//! Tickets, messages, unread markers, presence and the widget submit endpoint
//! (`docs/API.md` sections 7, 10, 11).

use std::collections::HashMap;
use std::str::FromStr;

use axum::Router;
use axum::extract::{FromRequestParts, Query};
use axum::http::request::Parts;
use axum::routing::{delete, get, post};
use dp_db::{MAX_LIMIT, Page};
use uuid::Uuid;

use crate::error::{ApiError, Fields};
use crate::state::AppState;

mod create;
mod dto;
mod messages;

pub use messages::reply_outbox_for;
mod read;
mod unread;
mod viewers;

pub use viewers::{spawn_gc, spawn_presence_gc};

const DEFAULT_LIMIT: i64 = 50;

pub fn router() -> Router<AppState> {
    let tickets = Router::new()
        .route("/", get(read::list).post(create::widget))
        .route("/admin", post(create::admin))
        .route("/counts", get(read::counts))
        .route("/search", get(read::search))
        .route(
            "/{id}",
            get(read::get).patch(read::update).delete(read::delete),
        )
        .route("/{id}/messages", get(messages::list).post(messages::create));
    let unread = Router::new().route("/", get(unread::list)).route(
        "/ticket/{id}",
        delete(unread::mark_read).post(unread::mark_unread),
    );
    let viewers = Router::new().route(
        "/{id}",
        get(viewers::list)
            .post(viewers::heartbeat)
            .delete(viewers::leave),
    );
    Router::new()
        .nest("/tickets", tickets)
        .nest("/unread", unread)
        // The UI calls `/unread/` with the trailing slash.
        .route("/unread/", get(unread::list))
        .nest("/viewers", viewers)
}

/// Query string as a map, so a bad value is a 422 naming the field instead of axum's plain-text 400.
struct Params(HashMap<String, String>);

impl<S: Send + Sync> FromRequestParts<S> for Params {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        Query::<HashMap<String, String>>::from_request_parts(parts, state)
            .await
            .map(|Query(m)| Self(m))
            .map_err(|_| ApiError::BadRequest("Malformed query string.".into()))
    }
}

impl Params {
    fn get(&self, key: &str) -> Option<&str> {
        self.0
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    /// Optional value parsed with `FromStr` (uuid, status, priority); a bad one is reported on `key`.
    fn parse<T: FromStr>(&self, key: &str, f: &mut Fields) -> Option<T> {
        let raw = self.get(key)?;
        raw.parse()
            .map_err(|_| f.add(key, &format!("Invalid {key}.")))
            .ok()
    }

    fn uuid(&self, key: &str, f: &mut Fields) -> Option<Uuid> {
        self.parse(key, f)
    }

    /// `limit` 1.., clamped to the maximum page; `offset` 0.. .
    fn page(&self, f: &mut Fields) -> Page {
        let mut num = |key: &str, default: i64, min: i64| match self.get(key) {
            None => default,
            Some(v) => match v.parse::<i64>() {
                Ok(n) if n >= min => n,
                _ => {
                    f.add(key, &format!("{key} must be an integer >= {min}."));
                    default
                }
            },
        };
        let limit = num("limit", DEFAULT_LIMIT, 1).min(MAX_LIMIT);
        let offset = num("offset", 0, 0);
        Page { limit, offset }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pairs: &[(&str, &str)]) -> Params {
        Params(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    #[test]
    fn page_defaults_and_clamps() {
        let mut f = Fields::default();
        let p = params(&[]).page(&mut f);
        assert_eq!((p.limit, p.offset), (50, 0));
        let p = params(&[("limit", "100000"), ("offset", "7")]).page(&mut f);
        assert_eq!((p.limit, p.offset), (MAX_LIMIT, 7));
        assert!(f.finish().is_ok());
    }

    #[test]
    fn page_rejects_garbage() {
        for (k, v) in [
            ("limit", "0"),
            ("limit", "-1"),
            ("limit", "x"),
            ("offset", "-1"),
        ] {
            let mut f = Fields::default();
            params(&[(k, v)]).page(&mut f);
            assert!(f.finish().is_err(), "{k}={v}");
        }
    }

    #[test]
    fn empty_values_count_as_absent() {
        let mut f = Fields::default();
        assert!(
            params(&[("projectId", "")])
                .uuid("projectId", &mut f)
                .is_none()
        );
        assert!(f.finish().is_ok());
        let mut f = Fields::default();
        assert!(
            params(&[("projectId", "nope")])
                .uuid("projectId", &mut f)
                .is_none()
        );
        assert!(f.finish().is_err());
    }
}
