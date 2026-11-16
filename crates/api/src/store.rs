//! Attachment storage handle: the filesystem driver (created on first use, because creating the
//! root is async) plus the signer for download links.

use dp_storage::{DownloadSigner, FsStorage};
use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;

use crate::config::Config;
use crate::error::ApiError;

pub struct Store {
    fs: OnceCell<FsStorage>,
    root: std::path::PathBuf,
    max_object: u64,
    pub signer: DownloadSigner,
}

impl Store {
    pub fn new(config: &Config) -> Self {
        // Separate key from the JWT secret, derived so no extra setting is needed.
        let mut h = Sha256::new();
        h.update(b"sd-download-signer-v1|");
        h.update(&config.jwt_secret);
        let key = h.finalize();
        Self {
            fs: OnceCell::new(),
            root: config.storage_dir.clone(),
            max_object: config.mail.max_file_bytes as u64,
            signer: DownloadSigner::new(&key).expect("sha256 output is 32 bytes"),
        }
    }

    pub async fn fs(&self) -> Result<&FsStorage, ApiError> {
        self.fs
            .get_or_try_init(|| FsStorage::new(&self.root, self.max_object))
            .await
            .map_err(ApiError::internal)
    }
}
