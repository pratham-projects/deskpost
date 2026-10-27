//! Argon2id hashing. Hashing is CPU and memory heavy, so it runs on the blocking pool behind a
//! semaphore: a burst of sign-ins queues instead of exhausting memory. Unknown emails are verified
//! against a dummy hash so they cost the same as a wrong password.

use argon2::password_hash::{PasswordHash, PasswordHasher as _, PasswordVerifier as _, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use tokio::sync::Semaphore;

use crate::config::{Argon2Cost, ConfigError};
use crate::error::ApiError;

pub struct Hasher {
    argon2: Argon2<'static>,
    permits: Semaphore,
    dummy: String,
}

impl Hasher {
    pub fn new(cost: Argon2Cost, concurrency: usize) -> Result<Self, ConfigError> {
        let bad = |e: &dyn std::fmt::Display| ConfigError(vec![format!("argon2 cost: {e}")]);
        let params = Params::new(cost.memory_kib, cost.iterations, cost.parallelism, None)
            .map_err(|e| bad(&e))?;
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut random = [0u8; 24];
        rand::fill(&mut random);
        let dummy = hash_sync(&argon2, &hex(&random)).map_err(|e| bad(&e))?;
        Ok(Self {
            argon2,
            permits: Semaphore::new(concurrency.max(1)),
            dummy,
        })
    }

    pub async fn hash(&self, password: String) -> Result<String, ApiError> {
        let _permit = self.permits.acquire().await.map_err(ApiError::internal)?;
        let argon2 = self.argon2.clone();
        tokio::task::spawn_blocking(move || hash_sync(&argon2, &password))
            .await
            .map_err(ApiError::internal)?
            .map_err(ApiError::internal)
    }

    /// `stored = None` (unknown account) burns the same time and always returns false.
    pub async fn verify(&self, password: String, stored: Option<String>) -> bool {
        let Ok(_permit) = self.permits.acquire().await else {
            return false;
        };
        let known = stored.is_some();
        let phc = stored.unwrap_or_else(|| self.dummy.clone());
        let argon2 = self.argon2.clone();
        let matched = tokio::task::spawn_blocking(move || {
            PasswordHash::new(&phc)
                .is_ok_and(|h| argon2.verify_password(password.as_bytes(), &h).is_ok())
        })
        .await
        .unwrap_or(false);
        known && matched
    }
}

fn hash_sync(argon2: &Argon2<'_>, password: &str) -> Result<String, argon2::password_hash::Error> {
    let mut salt = [0u8; 16];
    rand::fill(&mut salt);
    let salt = SaltString::encode_b64(&salt)?;
    Ok(argon2
        .hash_password(password.as_bytes(), &salt)?
        .to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
