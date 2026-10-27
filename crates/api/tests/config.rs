mod common;

use api::config::Config;
use common::{JWT_SECRET, SECRET_KEY, env_with};

fn errs(extra: &[(&str, &str)]) -> Vec<String> {
    Config::from_lookup(env_with(extra))
        .expect_err("config must be rejected")
        .0
}

fn rejects(extra: &[(&str, &str)], needle: &str) {
    let e = errs(extra);
    assert!(e.iter().any(|m| m.contains(needle)), "{needle}: {e:?}");
}

#[test]
fn valid_config_and_defaults() {
    let c = Config::from_lookup(env_with(&[])).unwrap();
    assert_eq!(c.port, 3000);
    assert_eq!(c.api_version, "v3");
    assert_eq!(c.body_limit, 64 * 1024);
    assert!(c.allow_signup);
    assert!(!c.production);
    assert_eq!(c.cors_origins, vec!["http://localhost:3001"]);
    assert_eq!(c.trusted_proxies.len(), 1);
}

#[test]
fn production_is_the_default_and_secures_cookies() {
    let get = |k: &str| match k {
        "DATABASE_URL" => Some("postgres://x".to_string()),
        "JWT_SECRET" => Some(JWT_SECRET.into()),
        "SD_SECRET_KEY" => Some(SECRET_KEY.into()),
        _ => None,
    };
    let c = Config::from_lookup(get).unwrap();
    assert!(c.production && c.cookie_secure);
    let c = Config::from_lookup(env_with(&[("COOKIE_SECURE", "true")])).unwrap();
    assert!(!c.production && c.cookie_secure);
}

#[test]
fn required_values_are_reported_together() {
    let e = Config::from_lookup(|_| None).unwrap_err().0;
    for name in ["DATABASE_URL", "JWT_SECRET", "SD_SECRET_KEY"] {
        assert!(e.iter().any(|m| m.contains(name)), "{name}: {e:?}");
    }
}

#[test]
fn short_secrets_are_rejected() {
    rejects(
        &[("JWT_SECRET", "short-but-random-Xq9")],
        "JWT_SECRET must be at least 32 bytes",
    );
    rejects(
        &[("SD_SECRET_KEY", "Zp7Qm2Xv9Lk4Nb8Tc3Wr6Yh1Gs5Df0J")],
        "SD_SECRET_KEY must be at least 32 bytes",
    );
    // 31 bytes: one short.
    rejects(&[("JWT_SECRET", &JWT_SECRET[..31])], "at least 32 bytes");
}

#[test]
fn known_placeholders_are_rejected() {
    for p in [
        "changeme-changeme-changeme-changeme",
        "your-secret-key-here-replace-me-now",
        "please_change_me_before_going_live_ok",
        "super-secret-jwt-token-do-not-share",
        "password1234567890password1234567890",
        "dummy-value-dummy-value-dummy-value-",
        "EXAMPLE_example_Example_ExAmPlE_1234",
    ] {
        rejects(&[("JWT_SECRET", p)], "JWT_SECRET looks like a placeholder");
        rejects(
            &[("SD_SECRET_KEY", p)],
            "SD_SECRET_KEY looks like a placeholder",
        );
    }
}

#[test]
fn low_entropy_secrets_are_rejected() {
    rejects(&[("JWT_SECRET", &"a".repeat(40))], "placeholder");
    rejects(&[("JWT_SECRET", &"abab".repeat(10))], "placeholder");
}

#[test]
fn the_two_secrets_must_differ() {
    rejects(&[("SD_SECRET_KEY", JWT_SECRET)], "must differ");
}

#[test]
fn bad_values_are_named() {
    rejects(&[("SD_ENV", "staging")], "SD_ENV");
    rejects(&[("PORT", "0")], "PORT");
    rejects(&[("PORT", "70000")], "PORT");
    rejects(&[("PORT", "abc")], "PORT");
    rejects(&[("API_VERSION", "v3/../x")], "API_VERSION");
    rejects(&[("API_VERSION", "v3;path")], "API_VERSION");
    rejects(&[("COOKIE_SECURE", "maybe")], "COOKIE_SECURE");
    rejects(&[("SD_BODY_LIMIT_BYTES", "0")], "SD_BODY_LIMIT_BYTES");
    rejects(
        &[("SD_UPLOAD_LIMIT_BYTES", "99999999999")],
        "SD_UPLOAD_LIMIT_BYTES",
    );
    rejects(
        &[("SD_TRUSTED_PROXIES", "10.0.0.0/8, not-a-cidr")],
        "not-a-cidr",
    );
    rejects(&[("SD_TRUSTED_PROXIES", "10.0.0.0/99")], "10.0.0.0/99");
}

#[test]
fn frontend_origins_must_be_plain_origins() {
    for bad in [
        "*",
        "app.example.com",
        "https://app.example.com/path",
        "https://a.com?x=1",
        "ftp://a.com",
        "https://u:p@a.com",
        "https://",
    ] {
        rejects(&[("FRONTEND_URL", bad)], "FRONTEND_URL");
    }
    let c = Config::from_lookup(env_with(&[(
        "FRONTEND_URL",
        "https://App.Example.com/, http://localhost:3000",
    )]))
    .unwrap();
    assert_eq!(
        c.cors_origins,
        vec!["https://app.example.com", "http://localhost:3000"]
    );
}

#[test]
fn trusted_proxies_accept_cidrs_and_bare_addresses() {
    let c = Config::from_lookup(env_with(&[(
        "SD_TRUSTED_PROXIES",
        "10.0.0.0/8, 192.168.1.5, ::1, fd00::/8",
    )]))
    .unwrap();
    assert_eq!(c.trusted_proxies.len(), 4);
    let c = Config::from_lookup(env_with(&[("SD_TRUSTED_PROXIES", "")])).unwrap();
    assert!(c.trusted_proxies.is_empty());
}

#[test]
fn debug_output_never_contains_secrets() {
    let c =
        Config::from_lookup(env_with(&[("DATABASE_URL", "postgres://u:hunter2@db/x")])).unwrap();
    let dbg = format!("{c:?}");
    for leak in [JWT_SECRET, SECRET_KEY, "hunter2"] {
        assert!(!dbg.contains(leak), "{dbg}");
    }
}
