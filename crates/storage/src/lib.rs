//! Attachment storage: a `Storage` trait, a traversal-safe filesystem driver, signed download
//! tokens and a quota helper. An S3 driver is future work (see `brain/architecture/storage.md`).

mod fs;
mod hex;
mod name;
mod token;

use std::fmt;
use std::future::Future;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncRead;

pub use fs::FsStorage;
pub use name::{sanitize_content_type, sanitize_filename};
pub use token::{Disposition, DownloadSigner, LEEWAY_SECS, TokenError};

/// Caller-supplied metadata. Never used to build a path.
#[derive(Debug, Clone, Default)]
pub struct PutMeta {
    pub original_filename: Option<String>,
    pub content_type: Option<String>,
}

/// What was stored. `key` is server generated; `sha256` is lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredObject {
    pub key: String,
    pub size: u64,
    pub sha256: String,
    pub original_filename: Option<String>,
    pub content_type: String,
}

#[derive(Debug)]
pub enum Error {
    /// Key is not in the server-generated shape.
    InvalidKey,
    /// A symlink or non-directory where the store expects a plain file or directory.
    Unsafe(&'static str),
    NotFound,
    TooLarge {
        max: u64,
    },
    Io(std::io::Error),
    /// Sidecar metadata unreadable.
    Meta(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidKey => write!(f, "invalid storage key"),
            Error::Unsafe(why) => write!(f, "unsafe storage path: {why}"),
            Error::NotFound => write!(f, "object not found"),
            Error::TooLarge { max } => write!(f, "object exceeds {max} bytes"),
            Error::Io(e) => write!(f, "storage io: {e}"),
            Error::Meta(e) => write!(f, "bad object metadata: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub trait Storage: Send + Sync {
    type Reader: AsyncRead + Send + Unpin;

    /// Stream `body` into a new object under a generated key. Aborts and cleans up past the size cap.
    fn put<R: AsyncRead + Send + Unpin>(
        &self,
        body: R,
        meta: PutMeta,
    ) -> impl Future<Output = Result<StoredObject>> + Send;

    fn get(&self, key: &str) -> impl Future<Output = Result<(Self::Reader, StoredObject)>> + Send;

    /// Idempotent: deleting a missing object is `Ok`.
    fn delete(&self, key: &str) -> impl Future<Output = Result<()>> + Send;

    fn exists(&self, key: &str) -> impl Future<Output = Result<bool>> + Send;
}

#[derive(Debug, PartialEq, Eq)]
pub struct QuotaExceeded {
    pub used: u64,
    pub incoming: u64,
    pub limit: u64,
}

/// `used` is the caller's sum of existing sizes for the project; overflow counts as exceeded.
pub fn check_quota(used: u64, incoming: u64, limit: u64) -> std::result::Result<(), QuotaExceeded> {
    match used.checked_add(incoming) {
        Some(total) if total <= limit => Ok(()),
        _ => Err(QuotaExceeded {
            used,
            incoming,
            limit,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn quota_boundaries() {
        assert!(check_quota(90, 10, 100).is_ok());
        assert_eq!(
            check_quota(90, 11, 100),
            Err(QuotaExceeded {
                used: 90,
                incoming: 11,
                limit: 100
            })
        );
        assert!(check_quota(u64::MAX, 1, u64::MAX).is_err());
    }

    proptest! {
        #[test]
        fn quota_matches_wide_math(used: u64, incoming: u64, limit: u64) {
            let ok = u128::from(used) + u128::from(incoming) <= u128::from(limit);
            prop_assert_eq!(check_quota(used, incoming, limit).is_ok(), ok);
        }
    }
}
