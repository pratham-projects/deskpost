//! Secret box: AES-256-GCM with a random 96-bit nonce per message, key derived from a
//! 32+ byte secret via HKDF-SHA256. Used for per-project SMTP/IMAP passwords at rest.
//!
//! Format: `v1:<base64url-nopad(nonce || ciphertext || tag)>`. The `aad` bound at seal time
//! (e.g. project id + field name) must be supplied again at open time, so a ciphertext copied
//! to another row or field fails authentication.

use std::fmt;

use aes_gcm::{
    Aes256Gcm, Key, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hkdf::Hkdf;
use sha2::Sha256;

const VERSION: &str = "v1:";
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const MIN_SECRET_LEN: usize = 32;
const HKDF_INFO: &[u8] = b"sendesk secret-box v1 aes-256-gcm";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretError {
    SecretTooShort,
    /// Not a `v1:` string, bad base64, or too short to hold nonce and tag.
    Malformed,
    /// Wrong key, wrong aad, or tampered ciphertext.
    Decrypt,
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::SecretTooShort => "secret must be at least 32 bytes",
            Self::Malformed => "malformed ciphertext",
            Self::Decrypt => "decryption failed",
        })
    }
}

impl std::error::Error for SecretError {}

pub struct SecretBox {
    cipher: Aes256Gcm,
}

impl fmt::Debug for SecretBox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretBox(..)")
    }
}

impl SecretBox {
    pub fn new(secret: &[u8]) -> Result<Self, SecretError> {
        if secret.len() < MIN_SECRET_LEN {
            return Err(SecretError::SecretTooShort);
        }
        let mut key = [0u8; 32];
        Hkdf::<Sha256>::new(None, secret)
            .expand(HKDF_INFO, &mut key)
            .expect("32 bytes is a valid HKDF-SHA256 output length");
        Ok(Self {
            cipher: Aes256Gcm::new(&Key::<Aes256Gcm>::from(key)),
        })
    }

    pub fn seal(&self, plaintext: &str, aad: &[u8]) -> String {
        let mut nonce = [0u8; NONCE_LEN];
        rand::fill(&mut nonce);
        let ct = self
            .cipher
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: plaintext.as_bytes(),
                    aad,
                },
            )
            .expect("AES-GCM encryption of an in-memory buffer cannot fail");
        let mut blob = Vec::with_capacity(NONCE_LEN + ct.len());
        blob.extend_from_slice(&nonce);
        blob.extend_from_slice(&ct);
        format!("{VERSION}{}", URL_SAFE_NO_PAD.encode(blob))
    }

    pub fn open(&self, sealed: &str, aad: &[u8]) -> Result<String, SecretError> {
        let b64 = sealed.strip_prefix(VERSION).ok_or(SecretError::Malformed)?;
        let blob = URL_SAFE_NO_PAD
            .decode(b64)
            .map_err(|_| SecretError::Malformed)?;
        if blob.len() < NONCE_LEN + TAG_LEN {
            return Err(SecretError::Malformed);
        }
        let (nonce, ct) = blob.split_at(NONCE_LEN);
        let nonce: [u8; NONCE_LEN] = nonce.try_into().map_err(|_| SecretError::Malformed)?;
        let pt = self
            .cipher
            .decrypt(&Nonce::from(nonce), Payload { msg: ct, aad })
            .map_err(|_| SecretError::Decrypt)?;
        String::from_utf8(pt).map_err(|_| SecretError::Decrypt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";

    fn boxed() -> SecretBox {
        SecretBox::new(SECRET).unwrap()
    }

    #[test]
    fn round_trip() {
        let b = boxed();
        let sealed = b.seal("hunter2", b"proj:1:smtp");
        assert!(sealed.starts_with("v1:"));
        assert!(!sealed.contains("hunter2"));
        assert_eq!(b.open(&sealed, b"proj:1:smtp").as_deref(), Ok("hunter2"));
    }

    #[test]
    fn empty_and_unicode_round_trip() {
        let b = boxed();
        for pt in ["", "pässwörd 🔑", "a\0b"] {
            assert_eq!(b.open(&b.seal(pt, b""), b"").as_deref(), Ok(pt));
        }
    }

    #[test]
    fn nonce_is_random_per_seal() {
        let b = boxed();
        assert_ne!(b.seal("x", b""), b.seal("x", b""));
    }

    #[test]
    fn short_secret_rejected() {
        assert!(matches!(
            SecretBox::new(&[7u8; 31]),
            Err(SecretError::SecretTooShort)
        ));
        assert!(SecretBox::new(&[7u8; 32]).is_ok());
    }

    #[test]
    fn wrong_secret_or_aad_fails() {
        let sealed = boxed().seal("pw", b"a");
        let other = SecretBox::new(b"ffffffffffffffffffffffffffffffff").unwrap();
        assert_eq!(other.open(&sealed, b"a"), Err(SecretError::Decrypt));
        assert_eq!(boxed().open(&sealed, b"b"), Err(SecretError::Decrypt));
    }

    #[test]
    fn malformed_inputs_are_rejected() {
        let b = boxed();
        for s in ["", "v1:", "v2:AAAA", "plain", "v1:!!!!", "v1:AAAA"] {
            assert_eq!(b.open(s, b""), Err(SecretError::Malformed), "{s:?}");
        }
    }

    #[test]
    fn debug_does_not_leak_key() {
        assert_eq!(format!("{:?}", boxed()), "SecretBox(..)");
    }

    proptest! {
        #[test]
        fn round_trips_any_string(pt in ".*", aad in proptest::collection::vec(any::<u8>(), 0..32)) {
            let b = boxed();
            prop_assert_eq!(b.open(&b.seal(&pt, &aad), &aad), Ok(pt));
        }

        #[test]
        fn any_single_byte_flip_is_detected(pt in ".{0,40}", idx in any::<usize>(), flip in 1u8..=255) {
            let b = boxed();
            let sealed = b.seal(&pt, b"");
            let raw = URL_SAFE_NO_PAD.decode(&sealed[3..]).unwrap();
            let mut tampered = raw.clone();
            let i = idx % tampered.len();
            tampered[i] ^= flip;
            let s = format!("v1:{}", URL_SAFE_NO_PAD.encode(tampered));
            prop_assert_eq!(b.open(&s, b""), Err(SecretError::Decrypt));
        }

        #[test]
        fn truncation_is_detected(pt in ".{0,40}", cut in 1usize..20) {
            let b = boxed();
            let sealed = b.seal(&pt, b"");
            let raw = URL_SAFE_NO_PAD.decode(&sealed[3..]).unwrap();
            let keep = raw.len().saturating_sub(cut);
            let s = format!("v1:{}", URL_SAFE_NO_PAD.encode(&raw[..keep]));
            prop_assert!(b.open(&s, b"").is_err());
        }

        #[test]
        fn open_never_panics(s in ".*") {
            let _ = boxed().open(&s, b"");
        }
    }
}
