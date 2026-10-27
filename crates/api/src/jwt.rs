//! HS256 access tokens. Only tokens this module issues verify: the header must match ours byte for
//! byte (so `alg: none` and algorithm switching are impossible), the MAC is checked in constant
//! time before the payload is parsed, and `exp`/`nbf` are enforced with no leeway.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;

/// `{"alg":"HS256","typ":"JWT"}`
const HEADER: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9";
const MAX_TOKEN_LEN: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    pub sub: Uuid,
    pub jti: Uuid,
    pub iat: i64,
    pub nbf: i64,
    pub exp: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JwtError {
    Invalid,
    Expired,
}

pub struct Jwt {
    key: Vec<u8>,
}

impl Jwt {
    pub fn new(secret: &[u8]) -> Self {
        Self {
            key: secret.to_vec(),
        }
    }

    fn mac(&self) -> Hmac<Sha256> {
        Hmac::<Sha256>::new_from_slice(&self.key).expect("HMAC accepts any key length")
    }

    pub fn issue(&self, user_id: Uuid, now: i64, ttl_secs: i64) -> String {
        let claims = Claims {
            sub: user_id,
            jti: Uuid::new_v4(),
            iat: now,
            nbf: now,
            exp: now + ttl_secs,
        };
        let payload = serde_json::to_vec(&claims).expect("claims serialize");
        let signing = format!("{HEADER}.{}", B64.encode(payload));
        let mut mac = self.mac();
        mac.update(signing.as_bytes());
        format!("{signing}.{}", B64.encode(mac.finalize().into_bytes()))
    }

    pub fn verify(&self, token: &str, now: i64) -> Result<Claims, JwtError> {
        if token.len() > MAX_TOKEN_LEN {
            return Err(JwtError::Invalid);
        }
        let mut parts = token.split('.');
        let (Some(header), Some(payload), Some(sig), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(JwtError::Invalid);
        };
        if header != HEADER {
            return Err(JwtError::Invalid);
        }
        let sig = B64.decode(sig).map_err(|_| JwtError::Invalid)?;
        let mut mac = self.mac();
        mac.update(header.as_bytes());
        mac.update(b".");
        mac.update(payload.as_bytes());
        // `verify_slice` compares in constant time.
        mac.verify_slice(&sig).map_err(|_| JwtError::Invalid)?;

        let bytes = B64.decode(payload).map_err(|_| JwtError::Invalid)?;
        let claims: Claims = serde_json::from_slice(&bytes).map_err(|_| JwtError::Invalid)?;
        if claims.nbf > now {
            return Err(JwtError::Invalid);
        }
        if claims.exp <= now {
            return Err(JwtError::Expired);
        }
        Ok(claims)
    }
}
