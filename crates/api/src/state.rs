use std::sync::Arc;

use sd_core::secret::SecretBox;
use sd_db::Pool;

use crate::config::{Config, ConfigError};
use crate::jwt::Jwt;
use crate::password::Hasher;
use crate::ratelimit::RateLimiter;

const RATE_LIMIT_KEYS: usize = 10_000;

#[derive(Clone)]
pub struct AppState {
    pub pool: Pool,
    pub config: Arc<Config>,
    pub jwt: Arc<Jwt>,
    pub hasher: Arc<Hasher>,
    pub limiter: Arc<RateLimiter>,
    /// Encrypts per-project mail passwords (R21+).
    pub secrets: Arc<SecretBox>,
}

impl AppState {
    pub fn new(pool: Pool, config: Config) -> Result<Self, ConfigError> {
        let secrets = SecretBox::new(&config.secret_key)
            .map_err(|e| ConfigError(vec![format!("SD_SECRET_KEY: {e}")]))?;
        let concurrency = std::thread::available_parallelism().map_or(2, |n| n.get().min(8));
        Ok(Self {
            pool,
            jwt: Arc::new(Jwt::new(&config.jwt_secret)),
            hasher: Arc::new(Hasher::new(config.argon2, concurrency)?),
            limiter: Arc::new(RateLimiter::new(RATE_LIMIT_KEYS)),
            secrets: Arc::new(secrets),
            config: Arc::new(config),
        })
    }
}
