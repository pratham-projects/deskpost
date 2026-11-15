//! dp-core: pure logic (no IO, no async). Statuses, email parsing, threading,
//! reply stripping, secret box, API keys, filename and content-type safety.

pub mod api_key;
pub mod body;
pub mod content_type;
pub mod email_addr;
pub mod filename;
pub mod secret;
pub mod status;
pub mod threading;

pub use ct::constant_time_eq;

mod ct {
    use sha2::{Digest, Sha256};
    use subtle::ConstantTimeEq;

    /// Constant-time equality that also hides the length difference by comparing SHA-256 digests.
    pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
        let (ha, hb) = (Sha256::digest(a), Sha256::digest(b));
        ha.ct_eq(&hb).into()
    }
}

#[cfg(test)]
mod tests {
    use super::constant_time_eq;
    use proptest::prelude::*;

    #[test]
    fn equal_and_unequal() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(constant_time_eq(b"", b""));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secret2"));
        assert!(!constant_time_eq(b"", b"x"));
    }

    proptest! {
        #[test]
        fn agrees_with_plain_equality(a in proptest::collection::vec(any::<u8>(), 0..64), b in proptest::collection::vec(any::<u8>(), 0..64)) {
            prop_assert_eq!(constant_time_eq(&a, &b), a == b);
        }
    }
}
