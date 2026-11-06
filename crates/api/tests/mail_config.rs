mod common;

use api::config::Config;
use common::*;

#[test]
fn defaults_match_the_contract() {
    let c = Config::from_lookup(env_with(&[])).unwrap().mail;
    assert!(c.webhook_secret.is_none() && !c.plaintext);
    assert_eq!(
        (c.max_file_bytes, c.max_request_bytes, c.max_files),
        (10 << 20, 25 << 20, 20)
    );
    assert_eq!(
        (c.imap_poll_interval.as_secs(), c.imap_concurrency),
        (60, 4)
    );
}

#[test]
fn settings_are_read_and_validated() {
    let ok = Config::from_lookup(env_with(&[
        ("SD_WEBHOOK_SECRET", "Qw3rTy9uIo1pAs5dFg7hJk2lZx4cVb6N"),
        ("SD_MAIL_PLAINTEXT", "1"),
        ("SD_MAX_FILES", "3"),
        ("SD_IMAP_POLL_SECS", "30"),
    ]))
    .unwrap()
    .mail;
    assert_eq!(
        ok.webhook_secret.as_deref(),
        Some(&b"Qw3rTy9uIo1pAs5dFg7hJk2lZx4cVb6N"[..])
    );
    assert!(ok.plaintext);
    assert_eq!((ok.max_files, ok.imap_poll_interval.as_secs()), (3, 30));

    for (k, v) in [
        ("SD_WEBHOOK_SECRET", "short"),
        ("SD_WEBHOOK_SECRET", "changeme-changeme-changeme-changeme"),
        ("SD_MAIL_PLAINTEXT", "maybe"),
        ("SD_MAX_FILES", "0"),
        ("SD_IMAP_POLL_SECS", "1"),
        ("SD_IMAP_CONCURRENCY", "1000"),
    ] {
        assert!(Config::from_lookup(env_with(&[(k, v)])).is_err(), "{k}={v}");
    }
}
