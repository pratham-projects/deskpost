//! Query-string extractor whose failures are envelopes, plus the shared `limit`/`offset` rules.

use std::collections::HashMap;

use axum::extract::{FromRequestParts, Query};
use axum::http::request::Parts;
use sd_db::{MAX_LIMIT, Page};

use crate::error::{ApiError, Fields};

pub struct Params(HashMap<String, String>);

impl<S: Send + Sync> FromRequestParts<S> for Params {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, ApiError> {
        Query::<HashMap<String, String>>::try_from_uri(&parts.uri)
            .map(|Query(m)| Self(m))
            .map_err(|_| ApiError::BadRequest("Malformed query string.".into()))
    }
}

impl Params {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    /// Required UUID query parameter (422 when absent or malformed).
    pub fn uuid(&self, key: &str, f: &mut Fields) -> Option<sd_db::Id> {
        match self.get(key).map(str::parse) {
            Some(Ok(id)) => Some(id),
            _ => {
                f.add(key, "A valid id is required.");
                None
            }
        }
    }

    /// `limit` (1..=200, default 50) and `offset` (>= 0); anything else is a 422.
    pub fn page(&self, f: &mut Fields) -> Page {
        let mut num = |key: &str, default: i64, min: i64, max: i64| match self.get(key) {
            None => default,
            Some(v) => match v.parse::<i64>() {
                Ok(n) if (min..=max).contains(&n) => n,
                _ => {
                    f.add(key, &format!("Must be an integer from {min} to {max}."));
                    default
                }
            },
        };
        let limit = num("limit", 50, 1, MAX_LIMIT);
        let offset = num("offset", 0, 0, i64::MAX);
        Page::new(limit, offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(q: &[(&str, &str)]) -> Params {
        Params(
            q.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    #[test]
    fn page_defaults_and_bounds() {
        let mut f = Fields::default();
        let p = params(&[]).page(&mut f);
        assert_eq!((p.limit, p.offset), (50, 0));
        assert!(f.finish().is_ok());
        for bad in [
            ("limit", "0"),
            ("limit", "201"),
            ("limit", "x"),
            ("offset", "-1"),
        ] {
            let mut f = Fields::default();
            params(&[bad]).page(&mut f);
            assert!(f.finish().is_err(), "{bad:?}");
        }
        let mut f = Fields::default();
        let p = params(&[("limit", "200"), ("offset", "7")]).page(&mut f);
        assert_eq!((p.limit, p.offset), (200, 7));
        assert!(f.finish().is_ok());
    }

    #[test]
    fn uuid_param() {
        let id = sd_db::new_id().to_string();
        let mut f = Fields::default();
        assert!(
            params(&[("projectId", &id)])
                .uuid("projectId", &mut f)
                .is_some()
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
