use api::jwt::{Claims, Jwt, JwtError};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use uuid::Uuid;

const SECRET: &[u8] = b"k3J9xQ2vL8mZ5nB7cR4wT6yU1aS0dFgH";
const NOW: i64 = 1_800_000_000;

fn jwt() -> Jwt {
    Jwt::new(SECRET)
}

fn sign(key: &[u8], signing_input: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).unwrap();
    mac.update(signing_input.as_bytes());
    B64.encode(mac.finalize().into_bytes())
}

fn forge(header: &str, claims: &Claims, key: &[u8]) -> String {
    let input = format!(
        "{}.{}",
        B64.encode(header),
        B64.encode(serde_json::to_vec(claims).unwrap())
    );
    format!("{input}.{}", sign(key, &input))
}

fn claims(sub: Uuid) -> Claims {
    Claims {
        sub,
        jti: Uuid::new_v4(),
        iat: NOW,
        nbf: NOW,
        exp: NOW + 300,
    }
}

#[test]
fn round_trip() {
    let id = Uuid::new_v4();
    let c = jwt().verify(&jwt().issue(id, NOW, 300), NOW + 1).unwrap();
    assert_eq!((c.sub, c.iat, c.nbf, c.exp), (id, NOW, NOW, NOW + 300));
}

#[test]
fn every_token_has_its_own_jti() {
    let id = Uuid::new_v4();
    let a = jwt().verify(&jwt().issue(id, NOW, 300), NOW).unwrap();
    let b = jwt().verify(&jwt().issue(id, NOW, 300), NOW).unwrap();
    assert_ne!(a.jti, b.jti);
}

#[test]
fn expiry_is_exclusive_and_has_no_leeway() {
    let t = jwt().issue(Uuid::new_v4(), NOW, 300);
    assert!(jwt().verify(&t, NOW + 299).is_ok());
    assert_eq!(jwt().verify(&t, NOW + 300), Err(JwtError::Expired));
    assert_eq!(jwt().verify(&t, NOW + 9_999), Err(JwtError::Expired));
}

#[test]
fn not_before_is_enforced() {
    let t = jwt().issue(Uuid::new_v4(), NOW, 300);
    assert_eq!(jwt().verify(&t, NOW - 1), Err(JwtError::Invalid));
}

#[test]
fn signed_with_another_key_is_invalid() {
    let t = forge(
        r#"{"alg":"HS256","typ":"JWT"}"#,
        &claims(Uuid::new_v4()),
        b"another-key-another-key-another-k",
    );
    assert_eq!(jwt().verify(&t, NOW), Err(JwtError::Invalid));
}

#[test]
fn tampered_payload_is_invalid() {
    let original = jwt().issue(Uuid::new_v4(), NOW, 300);
    let parts: Vec<&str> = original.split('.').collect();
    let evil = B64.encode(serde_json::to_vec(&claims(Uuid::new_v4())).unwrap());
    let t = format!("{}.{evil}.{}", parts[0], parts[2]);
    assert_eq!(jwt().verify(&t, NOW), Err(JwtError::Invalid));
}

#[test]
fn tampered_signature_is_invalid() {
    let t = jwt().issue(Uuid::new_v4(), NOW, 300);
    let (head, sig) = t.rsplit_once('.').unwrap();
    let mut raw = B64.decode(sig).unwrap();
    raw[0] ^= 1;
    assert_eq!(
        jwt().verify(&format!("{head}.{}", B64.encode(raw)), NOW),
        Err(JwtError::Invalid)
    );
    assert_eq!(
        jwt().verify(&format!("{head}."), NOW),
        Err(JwtError::Invalid)
    );
    assert_eq!(
        jwt().verify(&format!("{head}.{}", &sig[..sig.len() - 2]), NOW),
        Err(JwtError::Invalid)
    );
}

#[test]
fn alg_none_is_rejected() {
    let c = claims(Uuid::new_v4());
    let payload = B64.encode(serde_json::to_vec(&c).unwrap());
    for header in [
        r#"{"alg":"none","typ":"JWT"}"#,
        r#"{"alg":"None"}"#,
        r#"{"alg":"NONE"}"#,
    ] {
        let unsigned = format!("{}.{payload}.", B64.encode(header));
        assert_eq!(
            jwt().verify(&unsigned, NOW),
            Err(JwtError::Invalid),
            "{header}"
        );
        // Even with a valid MAC over the forged header the header itself is refused.
        assert_eq!(
            jwt().verify(&forge(header, &c, SECRET), NOW),
            Err(JwtError::Invalid),
            "{header}"
        );
    }
}

#[test]
fn other_algorithms_are_rejected() {
    let c = claims(Uuid::new_v4());
    for header in [
        r#"{"alg":"HS384","typ":"JWT"}"#,
        r#"{"alg":"RS256","typ":"JWT"}"#,
        r#"{"alg":"HS256"}"#,
    ] {
        assert_eq!(
            jwt().verify(&forge(header, &c, SECRET), NOW),
            Err(JwtError::Invalid),
            "{header}"
        );
    }
}

#[test]
fn malformed_tokens_are_invalid_not_panics() {
    let valid = jwt().issue(Uuid::new_v4(), NOW, 300);
    let cases = [
        String::new(),
        "abc".into(),
        "a.b".into(),
        format!("{valid}.extra"),
        "..".into(),
        "!!!.@@@.###".into(),
        "x".repeat(5_000),
        format!("{valid}\0"),
    ];
    for t in cases {
        assert_eq!(jwt().verify(&t, NOW), Err(JwtError::Invalid), "{t:.40}");
    }
}

#[test]
fn signed_non_claims_payload_is_invalid() {
    let input = format!(
        "{}.{}",
        B64.encode(r#"{"alg":"HS256","typ":"JWT"}"#),
        B64.encode("not json")
    );
    let t = format!("{input}.{}", sign(SECRET, &input));
    assert_eq!(jwt().verify(&t, NOW), Err(JwtError::Invalid));
}
