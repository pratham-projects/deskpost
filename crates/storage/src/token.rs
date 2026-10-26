use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::hex;

type HmacSha256 = Hmac<Sha256>;

/// Clock skew accepted past `expires_at`.
pub const LEEWAY_SECS: u64 = 30;
const MIN_SECRET: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Inline,
    Attachment,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TokenError {
    WeakSecret,
    Malformed,
    BadSignature,
    Expired,
}

/// Token = `<expires_at>.<i|a>.<hex hmac>`; the key is not in the token, the caller supplies it
/// at verify time so a token only works for the object it was minted for.
pub struct DownloadSigner {
    secret: Vec<u8>,
}

impl DownloadSigner {
    pub fn new(secret: &[u8]) -> Result<Self, TokenError> {
        if secret.len() < MIN_SECRET {
            return Err(TokenError::WeakSecret);
        }
        Ok(Self {
            secret: secret.to_vec(),
        })
    }

    fn mac(&self, key: &str, expires_at: u64, d: char) -> HmacSha256 {
        let mut mac = HmacSha256::new_from_slice(&self.secret).expect("hmac takes any key length");
        // Length prefix keeps the field boundaries unambiguous whatever the key contains.
        mac.update(format!("sd-dl-v1|{}|{key}|{expires_at}|{d}", key.len()).as_bytes());
        mac
    }

    pub fn sign(&self, key: &str, disposition: Disposition, expires_at: u64) -> String {
        let d = tag(disposition);
        let sig = self.mac(key, expires_at, d).finalize().into_bytes();
        format!("{expires_at}.{d}.{}", hex::encode(&sig))
    }

    /// `now` is unix seconds. Signature is checked (constant time) before expiry.
    pub fn verify(&self, key: &str, token: &str, now: u64) -> Result<Disposition, TokenError> {
        let mut parts = token.split('.');
        let (Some(exp), Some(d), Some(sig), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(TokenError::Malformed);
        };
        let expires_at: u64 = exp.parse().map_err(|_| TokenError::Malformed)?;
        if expires_at.to_string() != exp {
            return Err(TokenError::Malformed);
        }
        let disposition = match d {
            "i" => Disposition::Inline,
            "a" => Disposition::Attachment,
            _ => return Err(TokenError::Malformed),
        };
        let sig = hex::decode(sig).ok_or(TokenError::Malformed)?;
        self.mac(key, expires_at, tag(disposition))
            .verify_slice(&sig)
            .map_err(|_| TokenError::BadSignature)?;
        if now > expires_at.saturating_add(LEEWAY_SECS) {
            return Err(TokenError::Expired);
        }
        Ok(disposition)
    }
}

fn tag(d: Disposition) -> char {
    match d {
        Disposition::Inline => 'i',
        Disposition::Attachment => 'a',
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const KEY: &str = "ab/cd/abcd0000000000000000000000000000";

    fn signer() -> DownloadSigner {
        DownloadSigner::new(&[7u8; 32]).unwrap()
    }

    #[test]
    fn weak_secret_rejected() {
        assert_eq!(
            DownloadSigner::new(&[1u8; 31]).err(),
            Some(TokenError::WeakSecret)
        );
    }

    #[test]
    fn valid_token_roundtrip() {
        let s = signer();
        let t = s.sign(KEY, Disposition::Inline, 1000);
        assert_eq!(s.verify(KEY, &t, 999), Ok(Disposition::Inline));
        let t = s.sign(KEY, Disposition::Attachment, 1000);
        assert_eq!(s.verify(KEY, &t, 1000), Ok(Disposition::Attachment));
    }

    #[test]
    fn expiry_with_leeway() {
        let s = signer();
        let t = s.sign(KEY, Disposition::Attachment, 1000);
        assert!(s.verify(KEY, &t, 1000 + LEEWAY_SECS).is_ok());
        assert_eq!(
            s.verify(KEY, &t, 1000 + LEEWAY_SECS + 1),
            Err(TokenError::Expired)
        );
    }

    #[test]
    fn tamper_each_field() {
        let s = signer();
        let t = s.sign(KEY, Disposition::Attachment, 1000);
        let (exp, rest) = t.split_once('.').unwrap();
        let (_, sig) = rest.split_once('.').unwrap();
        // disposition flipped
        assert_eq!(
            s.verify(KEY, &format!("{exp}.i.{sig}"), 0),
            Err(TokenError::BadSignature)
        );
        // expiry extended
        assert_eq!(
            s.verify(KEY, &format!("9999.a.{sig}"), 0),
            Err(TokenError::BadSignature)
        );
        // other key
        let other = "ab/cd/abcd1111111111111111111111111111";
        assert_eq!(s.verify(other, &t, 0), Err(TokenError::BadSignature));
        // flipped signature nibble
        let mut bad = t.clone().into_bytes();
        let last = bad.len() - 1;
        bad[last] = if bad[last] == b'0' { b'1' } else { b'0' };
        assert_eq!(
            s.verify(KEY, &String::from_utf8(bad).unwrap(), 0),
            Err(TokenError::BadSignature)
        );
        // other secret
        let s2 = DownloadSigner::new(&[8u8; 32]).unwrap();
        assert_eq!(s2.verify(KEY, &t, 0), Err(TokenError::BadSignature));
    }

    #[test]
    fn malformed() {
        let s = signer();
        for t in [
            "",
            "x",
            "1.a",
            "1.a.zz",
            "+1000.a.00",
            "01000.a.00",
            "1.x.00",
            "1.a.00.00",
            "1.a.AB",
        ] {
            assert_eq!(s.verify(KEY, t, 0), Err(TokenError::Malformed), "{t}");
        }
    }

    proptest! {
        #[test]
        fn never_panics_on_garbage(t in ".*") {
            let _ = signer().verify(KEY, &t, 0);
        }

        #[test]
        fn roundtrip_any(key in ".*", exp in 0u64..u64::MAX / 2, inline: bool) {
            let s = signer();
            let d = if inline { Disposition::Inline } else { Disposition::Attachment };
            prop_assert_eq!(s.verify(&key, &s.sign(&key, d, exp), exp), Ok(d));
        }
    }
}
