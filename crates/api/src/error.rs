//! Response envelope and error type from `docs/API.md` section 1.

use std::collections::BTreeMap;

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, PartialEq)]
pub enum ApiError {
    BadRequest(String),
    TokenMissing,
    TokenExpired,
    TokenInvalid,
    InvalidCredentials,
    Forbidden(String),
    NotFound(String),
    MethodNotAllowed,
    Conflict(String),
    PayloadTooLarge,
    UnsupportedMediaType,
    /// `field -> message`
    Validation(BTreeMap<String, String>),
    RateLimited {
        retry_after_secs: u64,
    },
    /// Details are logged where the error is created, never sent to the client.
    /// Outbound SMTP failed (`502 MAIL_DELIVERY_FAILED`).
    MailDeliveryFailed,
    Internal,
}

impl ApiError {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::TokenMissing
            | Self::TokenExpired
            | Self::TokenInvalid
            | Self::InvalidCredentials => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            Self::MailDeliveryFailed => StatusCode::BAD_GATEWAY,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::BadRequest(_) => "BAD_REQUEST",
            Self::TokenMissing => "TOKEN_MISSING",
            Self::TokenExpired => "TOKEN_EXPIRED",
            Self::TokenInvalid => "TOKEN_INVALID",
            Self::InvalidCredentials => "INVALID_CREDENTIALS",
            Self::Forbidden(_) => "FORBIDDEN",
            Self::NotFound(_) => "NOT_FOUND",
            Self::MethodNotAllowed => "METHOD_NOT_ALLOWED",
            Self::Conflict(_) => "CONFLICT",
            Self::PayloadTooLarge => "PAYLOAD_TOO_LARGE",
            Self::UnsupportedMediaType => "UNSUPPORTED_MEDIA_TYPE",
            Self::Validation(_) => "VALIDATION_ERROR",
            Self::RateLimited { .. } => "RATE_LIMITED",
            Self::MailDeliveryFailed => "MAIL_DELIVERY_FAILED",
            Self::Internal => "INTERNAL",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::BadRequest(m) | Self::Forbidden(m) | Self::NotFound(m) | Self::Conflict(m) => {
                m.clone()
            }
            Self::MethodNotAllowed => "Method not allowed.".into(),
            Self::TokenMissing => "Authentication required.".into(),
            Self::TokenExpired => "Token expired.".into(),
            Self::TokenInvalid => "Invalid token.".into(),
            Self::InvalidCredentials => "Invalid email or password.".into(),
            Self::PayloadTooLarge => "Request body too large.".into(),
            Self::UnsupportedMediaType => "Unsupported media type.".into(),
            Self::Validation(_) => "Validation failed.".into(),
            Self::RateLimited { .. } => "Too many requests. Try again later.".into(),
            Self::MailDeliveryFailed => "The email could not be delivered.".into(),
            Self::Internal => "Internal server error.".into(),
        }
    }

    pub fn not_found(what: &str) -> Self {
        Self::NotFound(format!("{what} not found."))
    }

    /// A database failure is a server fault: log it, tell the client nothing.
    pub fn internal(err: impl std::fmt::Display) -> Self {
        tracing::error!(error = %err, "internal error");
        Self::Internal
    }
}

impl From<dp_db::Error> for ApiError {
    fn from(e: dp_db::Error) -> Self {
        Self::internal(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut error = json!({ "code": self.code(), "message": self.message() });
        if let Self::Validation(fields) = &self {
            error["details"] = json!(fields);
        }
        let mut res = (
            self.status(),
            Json(json!({ "success": false, "error": error })),
        )
            .into_response();
        match &self {
            Self::RateLimited { retry_after_secs } => {
                res.headers_mut()
                    .insert(header::RETRY_AFTER, HeaderValue::from(*retry_after_secs));
            }
            e if e.status() == StatusCode::UNAUTHORIZED => {
                res.headers_mut()
                    .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            }
            _ => {}
        }
        res
    }
}

/// Collects `field -> message` pairs so a request reports every bad field at once.
#[derive(Default)]
pub struct Fields(BTreeMap<String, String>);

impl Fields {
    pub fn add(&mut self, field: &str, message: &str) {
        self.0.entry(field.into()).or_insert_with(|| message.into());
    }

    pub fn finish(self) -> Result<(), ApiError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(ApiError::Validation(self.0))
        }
    }
}

#[derive(Serialize)]
pub struct Pagination {
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
    #[serde(rename = "hasMore")]
    pub has_more: bool,
}

impl Pagination {
    pub fn new(total: i64, limit: i64, offset: i64) -> Self {
        Self {
            total,
            limit,
            offset,
            has_more: offset.saturating_add(limit) < total,
        }
    }
}

/// Success envelope. `pagination` and `stats` are only present on the endpoints that define them.
#[derive(Serialize)]
pub struct Envelope<T: Serialize> {
    success: bool,
    data: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    pagination: Option<Pagination>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stats: Option<Value>,
    #[serde(skip)]
    status: StatusCode,
}

impl<T: Serialize> Envelope<T> {
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            data,
            pagination: None,
            stats: None,
            status: StatusCode::OK,
        }
    }

    pub fn created(data: T) -> Self {
        Self {
            status: StatusCode::CREATED,
            ..Self::ok(data)
        }
    }

    pub fn paginated(mut self, p: Pagination) -> Self {
        self.pagination = Some(p);
        self
    }

    pub fn stats(mut self, stats: Value) -> Self {
        self.stats = Some(stats);
        self
    }
}

impl<T: Serialize> IntoResponse for Envelope<T> {
    fn into_response(self) -> Response {
        let status = self.status;
        (status, Json(self)).into_response()
    }
}

#[cfg(test)]
mod pagination_tests {
    use super::Pagination;

    #[test]
    fn has_more_cannot_overflow() {
        assert!(!Pagination::new(10, 50, i64::MAX).has_more);
        assert!(!Pagination::new(10, 200, i64::MAX - 1).has_more);
        assert!(Pagination::new(10, 5, 0).has_more);
        assert!(!Pagination::new(10, 5, 5).has_more);
    }
}
