//! Project API keys: random, shown once, stored only as a SHA-256 hash.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

use crate::constant_time_eq;

const PREFIX: &str = "sdk_";

/// Returns `(plaintext, sha256_hex)`. The plaintext is 256 bits of randomness.
pub fn generate_api_key() -> (String, String) {
    let mut raw = [0u8; 32];
    rand::fill(&mut raw);
    let key = format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(raw));
    let hash = hash_api_key(&key);
    (key, hash)
}

/// Lowercase hex SHA-256 of the key.
pub fn hash_api_key(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Constant-time check of a presented key against a stored hash.
pub fn verify_api_key(presented: &str, stored_hash: &str) -> bool {
    constant_time_eq(hash_api_key(presented).as_bytes(), stored_hash.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn generated_key_verifies_and_has_shape() {
        let (key, hash) = generate_api_key();
        assert!(key.starts_with("sdk_"));
        assert_eq!(key.len(), 4 + 43);
        assert_eq!(hash.len(), 64);
        assert!(
            hash.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        assert!(verify_api_key(&key, &hash));
    }

    #[test]
    fn keys_are_unique() {
        let (a, _) = generate_api_key();
        let (b, _) = generate_api_key();
        assert_ne!(a, b);
    }

    #[test]
    fn wrong_key_and_garbage_hash_fail() {
        let (key, hash) = generate_api_key();
        assert!(!verify_api_key(&format!("{key}x"), &hash));
        assert!(!verify_api_key(&key, ""));
        assert!(!verify_api_key(&key, "zz"));
        assert!(!verify_api_key("", &hash));
    }

    #[test]
    fn known_sha256_vector() {
        assert_eq!(
            hash_api_key("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hash_is_not_the_key() {
        let (key, hash) = generate_api_key();
        assert!(!hash.contains(&key));
    }

    proptest! {
        #[test]
        fn verify_accepts_only_the_hashed_input(a in ".{0,64}", b in ".{0,64}") {
            let h = hash_api_key(&a);
            prop_assert!(verify_api_key(&a, &h));
            prop_assert_eq!(verify_api_key(&b, &h), a == b);
        }
    }
}
