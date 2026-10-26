//! Email address extraction, validation and normalisation (port of `emailParser.ts`).
//!
//! Deliberate differences from the TypeScript product:
//! - RFC 2047 encoded words are *removed*, never decoded, before extraction. Decoding first let a
//!   display name such as `=?UTF-8?B?...?=` expand into `evil@x.com <...>` and spoof the sender.
//! - An address in angle brackets always beats one in the display name or a group list.
//! - `is_valid_email` is stricter than `^[^\s@]+@[^\s@]+\.[^\s@]+$` (no `a@b..com`, `a b@c.com`,
//!   control characters, header-injection characters, over-long parts).
//! - Domains are lowercased and IDNs converted to punycode.

use std::sync::LazyLock;

use regex::Regex;

/// Inputs longer than this are cut (on a char boundary) before parsing.
const MAX_INPUT: usize = 4096;

static ENCODED_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"=\?[^?\s]+\?[BbQq]\?[^?\s]*\?=").unwrap());
static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\([^()]*\)").unwrap());
static ANGLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<([^<>]*)>").unwrap());
static GROUP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r":\s*([^;]+);").unwrap());
static BARE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\p{L}\p{N}._%+\-]+@[\p{L}\p{N}.\-]+\.\p{L}{2,}").unwrap());

fn clip(s: &str) -> &str {
    let mut end = MAX_INPUT.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn strip_comments(s: &str) -> String {
    // Nested comments: peel innermost pairs until nothing changes (each pass shrinks the string).
    let mut cur = s.to_string();
    loop {
        let next = COMMENT.replace_all(&cur, "").into_owned();
        if next == cur {
            return next.trim().to_string();
        }
        cur = next;
    }
}

/// Drops RFC 5322 quoted-strings (honouring backslash escapes) so text inside a display name
/// can never be mistaken for the real angle-addr.
fn mask_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let (mut in_q, mut esc) = (false, false);
    for c in s.chars() {
        match (in_q, esc, c) {
            (_, true, _) => esc = false,
            (_, false, '\\') => esc = true,
            (false, _, '"') => in_q = true,
            (true, _, '"') => in_q = false,
            (false, _, _) => out.push(c),
            _ => {}
        }
    }
    out
}

fn to_ascii_domain(addr: &str) -> String {
    let Some((local, domain)) = addr.rsplit_once('@') else {
        return addr.to_string();
    };
    match idna::domain_to_ascii(domain) {
        Ok(d) if !d.is_empty() => format!("{local}@{d}"),
        _ => addr.to_string(),
    }
}

/// Extracts the bare address from a header value; returns the trimmed input when none is found.
/// Null sender (`<>`) and empty input give `""`.
pub fn extract_email_address(input: &str) -> String {
    let trimmed = clip(input).trim();
    if trimmed.is_empty() || trimmed == "<>" {
        return String::new();
    }

    let no_words = ENCODED_WORD.replace_all(trimmed, "");
    let no_comments = strip_comments(&no_words);
    let s = no_comments.replace("\\\"", "\"").replace("\\'", "'");
    let s = s.trim();

    let masked = mask_quoted(&no_comments);
    let mut found = ANGLE
        .captures_iter(&masked)
        .map(|c| c[1].trim().to_string())
        .find(|a| !a.is_empty())
        .map(|a| a.replace("\\\"", "\""));

    if found.is_none() {
        let scope = GROUP
            .captures(s)
            .map(|c| c[1].split(',').next().unwrap_or("").trim().to_string())
            .filter(|g| !g.is_empty());
        let scope = scope.as_deref().unwrap_or(s);
        found = BARE.find(scope).map(|m| m.as_str().to_string());
    }

    let mut out = found.unwrap_or_else(|| s.to_string());
    if let Some(inner) = out
        .strip_prefix(['"', '\''])
        .and_then(|r| r.strip_suffix(['"', '\'']))
        .filter(|i| !i.is_empty())
    {
        out = inner.trim().to_string();
    }
    if out.contains('@') {
        out = to_ascii_domain(&out);
        if let Some((local, domain)) = out.rsplit_once('@') {
            out = format!("{local}@{}", domain.to_ascii_lowercase());
        }
    }
    out.trim().to_string()
}

fn is_atext(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+/=?^_`{|}~-".contains(&b)
}

/// Strict, practical validation of a bare address (ASCII domain; use after extraction).
pub fn is_valid_email(email: &str) -> bool {
    if email.is_empty() || email.len() > 254 {
        return false;
    }
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    if local.is_empty() || local.len() > 64 || local.starts_with('.') || local.ends_with('.') {
        return false;
    }
    if local.contains("..") || !local.bytes().all(|b| is_atext(b) || b == b'.') {
        return false;
    }
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    let label_ok = |l: &&str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    labels.iter().all(label_ok) && labels.last().is_some_and(|tld| tld.len() >= 2)
}

/// Extract and validate; `None` when no valid address is present.
pub fn extract_and_validate_email(input: &str) -> Option<String> {
    let e = extract_email_address(input);
    is_valid_email(&e).then_some(e)
}

/// Canonical lookup key for a customer/user address: extracted, validated and lowercased.
pub fn normalize_email(input: &str) -> Option<String> {
    extract_and_validate_email(input).map(|e| e.to_ascii_lowercase())
}

#[derive(Debug, Default, Clone, Copy)]
pub struct EmailHeaders<'a> {
    pub reply_to: Option<&'a str>,
    pub sender: Option<&'a str>,
    pub from: &'a str,
    pub return_path: Option<&'a str>,
}

/// Address to answer: Reply-To, then Sender, then From, then Return-Path; first *valid* wins.
/// Falls back to the raw `from` value.
pub fn response_email(h: &EmailHeaders<'_>) -> String {
    [h.reply_to, h.sender, Some(h.from), h.return_path]
        .into_iter()
        .flatten()
        .find_map(extract_and_validate_email)
        .unwrap_or_else(|| h.from.to_string())
}

const BOUNCE_LOCALS: [&str; 7] = [
    "mailer-daemon",
    "postmaster",
    "bounce",
    "noreply",
    "no-reply",
    "donotreply",
    "do-not-reply",
];

/// Null sender or a system/no-reply mailbox: never create a ticket or auto-reply to these.
pub fn is_bounce_message(envelope_from: &str) -> bool {
    let addr = extract_email_address(envelope_from);
    if addr.is_empty() {
        return true;
    }
    let local = addr.split('@').next().unwrap_or("").to_ascii_lowercase();
    addr.contains('@') && BOUNCE_LOCALS.iter().any(|b| local == *b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn ex(s: &str) -> String {
        extract_email_address(s)
    }

    // ---- ported from emailParser.test.ts: extractEmailAddress ----

    #[test]
    fn angle_bracket_formats() {
        assert_eq!(
            ex(r#""qwerty" <q97159745@gmail.com>"#),
            "q97159745@gmail.com"
        );
        assert_eq!(ex("John Doe <john@example.com>"), "john@example.com");
        assert_eq!(ex("<support@company.com>"), "support@company.com");
        assert_eq!(ex("Name < user@test.com >"), "user@test.com");
        assert_eq!(
            ex(r#""Smith, John (CEO)" <ceo@company.com>"#),
            "ceo@company.com"
        );
    }

    #[test]
    fn rfc2047_encoded_display_names() {
        assert_eq!(ex("=?UTF-8?B?Sm9zw6k=?= <jose@test.com>"), "jose@test.com");
        assert_eq!(
            ex("=?UTF-8?Q?Jos=C3=A9?= <jose@example.com>"),
            "jose@example.com"
        );
        assert_eq!(
            ex("=?UTF-8?B?Sm9zw6k=?= =?UTF-8?B?R8OzbWV6?= <jose@test.com>"),
            "jose@test.com"
        );
        assert_eq!(ex("John Smith <john@test.com>"), "john@test.com");
    }

    #[test]
    fn encoded_word_cannot_inject_an_address() {
        // base64 of "evil@x.com <evil@x.com>"
        let evil = "=?UTF-8?B?ZXZpbEB4LmNvbSA8ZXZpbEB4LmNvbT4=?= <real@test.com>";
        assert_eq!(ex(evil), "real@test.com");
        // an encoded word with no real address must not produce one either
        assert!(!ex("=?UTF-8?B?ZXZpbEB4LmNvbQ==?= nobody").contains("evil"));
    }

    #[test]
    fn comments_in_parentheses() {
        assert_eq!(
            ex("John Smith (CEO) <john@example.com>"),
            "john@example.com"
        );
        assert_eq!(ex("John (VP (Sales)) <john@test.com>"), "john@test.com");
        assert_eq!(
            ex("John (CEO) Smith (Founder) <john@test.com>"),
            "john@test.com"
        );
    }

    #[test]
    fn multiple_addresses_take_first() {
        assert_eq!(ex("Alice <alice@a.com>, Bob <bob@b.com>"), "alice@a.com");
        assert_eq!(ex("A <a@a.com>, B <b@b.com>, C <c@c.com>"), "a@a.com");
    }

    #[test]
    fn group_syntax() {
        assert_eq!(ex("Team: a@x.com, b@x.com;"), "a@x.com");
        assert_eq!(ex("Team: Alice <a@x.com>, Bob <b@x.com>;"), "a@x.com");
    }

    #[test]
    fn missing_angle_brackets() {
        assert_eq!(ex("John Smith john@example.com"), "john@example.com");
        assert_eq!(ex("simple@example.com"), "simple@example.com");
    }

    #[test]
    fn broken_brackets() {
        assert_eq!(ex("<<john@example.com>>"), "john@example.com");
        assert_eq!(ex("<john@example.com"), "john@example.com");
        assert_eq!(ex("john@example.com>"), "john@example.com");
        assert_eq!(ex("<<<user@test.com>>>"), "user@test.com");
    }

    #[test]
    fn nested_and_escaped_quotes() {
        assert_eq!(
            ex(r#""{\"John\"} Smith" <john@example.com>"#),
            "john@example.com"
        );
        assert_eq!(ex(r#"\"John\" <john@test.com>"#), "john@test.com");
    }

    #[test]
    fn internationalized_domains() {
        assert_eq!(ex("user@xn--bcher-kva.de"), "user@xn--bcher-kva.de");
        assert_eq!(ex("user@bücher.de"), "user@xn--bcher-kva.de");
        assert_eq!(ex("user@example.com"), "user@example.com");
    }

    #[test]
    fn plus_addressing() {
        assert_eq!(ex("user+tag@gmail.com"), "user+tag@gmail.com");
        assert_eq!(ex("John <user+tag@gmail.com>"), "user+tag@gmail.com");
    }

    #[test]
    fn null_sender() {
        assert_eq!(ex("<>"), "");
        assert_eq!(ex(""), "");
        assert_eq!(ex("   "), "");
    }

    #[test]
    fn display_name_contains_email() {
        assert_eq!(
            ex("john@example.com <real@company.com>"),
            "real@company.com"
        );
    }

    #[test]
    fn quoted_formats() {
        assert_eq!(ex(r#""user@example.com""#), "user@example.com");
        assert_eq!(ex("'user@example.com'"), "user@example.com");
    }

    #[test]
    fn clean_formats() {
        assert_eq!(ex("simple@example.com"), "simple@example.com");
        assert_eq!(ex("  user@test.com  "), "user@test.com");
    }

    #[test]
    fn malformed_angle_brackets_return_input() {
        assert_eq!(ex("Name <incomplete"), "Name <incomplete");
    }

    #[test]
    fn real_world_examples() {
        assert_eq!(
            ex(r#""John Smith" <john.smith@gmail.com>"#),
            "john.smith@gmail.com"
        );
        assert_eq!(
            ex("Smith, John <john.smith@company.com>"),
            "john.smith@company.com"
        );
        assert_eq!(ex("John Smith <john@icloud.com>"), "john@icloud.com");
        assert_eq!(
            ex(r#""François Müller" <francois@example.fr>"#),
            "francois@example.fr"
        );
        assert_eq!(ex("Marketing Team <news@company.com>"), "news@company.com");
        assert_eq!(ex("noreply@company.com"), "noreply@company.com");
    }

    #[test]
    fn obfuscated_formats_do_not_crash_or_validate() {
        let r = ex("john at example dot com");
        assert_eq!(r, "john at example dot com");
        assert!(!is_valid_email(&r));
    }

    // ---- new behaviour ----

    #[test]
    fn domain_is_lowercased_local_part_kept() {
        assert_eq!(ex("John <John.Doe@EXAMPLE.Com>"), "John.Doe@example.com");
        assert_eq!(
            normalize_email("John <John.Doe@EXAMPLE.Com>").as_deref(),
            Some("john.doe@example.com")
        );
    }

    #[test]
    fn bracketed_address_beats_group_text() {
        assert_eq!(ex("Sales: fake@evil.com; <real@ok.com>"), "real@ok.com");
    }

    #[test]
    fn angle_inside_quoted_display_name_cannot_spoof() {
        assert_eq!(
            ex(r#""boss@corp.com <boss@corp.com>" <real@evil.com>"#),
            "real@evil.com"
        );
        assert_eq!(ex(r#""a \" <x@y.com>" <real@ok.com>"#), "real@ok.com");
        assert_eq!(ex(r#"<real@ok.com> "x <fake@y.com>""#), "real@ok.com");
    }

    #[test]
    fn oversized_input_is_bounded() {
        let big = format!("{} <a@b.com>", "x".repeat(100_000));
        let _ = ex(&big);
        let nested = "(".repeat(50_000);
        let _ = ex(&nested);
    }

    // ---- isValidEmail ----

    #[test]
    fn valid_emails() {
        for e in [
            "user@example.com",
            "user@mail.example.com",
            "user+tag@example.com",
            "first.last@example.com",
            "user123@example456.com",
            "user@xn--bcher-kva.de",
        ] {
            assert!(is_valid_email(e), "{e}");
        }
    }

    #[test]
    fn invalid_emails() {
        for e in [
            "userexample.com",
            "user@",
            "@example.com",
            "user@example",
            "user name@example.com",
            "",
            "a@b..com",
            ".a@b.com",
            "a.@b.com",
            "a..b@c.com",
            "a@-b.com",
            "a@b-.com",
            "a@b.c",
            "a@@b.com",
            "a\r\nBcc: x@y.com@b.com",
            "a<b>@c.com",
            "a,b@c.com",
            "a@b.com\n",
        ] {
            assert!(!is_valid_email(e), "{e:?}");
        }
        assert!(!is_valid_email(&format!("{}@b.com", "a".repeat(65))));
        assert!(!is_valid_email(&format!("a@{}.com", "b".repeat(64))));
        assert!(!is_valid_email(&format!("a@{}.com", "b.".repeat(130))));
    }

    // ---- extractAndValidateEmail ----

    #[test]
    fn extract_and_validate() {
        assert_eq!(
            extract_and_validate_email(r#""John" <john@example.com>"#).as_deref(),
            Some("john@example.com")
        );
        assert_eq!(
            extract_and_validate_email(r#""John" <invalid-email>"#),
            None
        );
        assert_eq!(extract_and_validate_email(""), None);
        assert_eq!(
            extract_and_validate_email("user@test.com").as_deref(),
            Some("user@test.com")
        );
    }

    // ---- getResponseEmail ----

    #[test]
    fn response_prefers_reply_to() {
        let h = EmailHeaders {
            reply_to: Some("support@company.com"),
            sender: Some("bounce@mailer.com"),
            from: "noreply@company.com",
            ..Default::default()
        };
        assert_eq!(response_email(&h), "support@company.com");
    }

    #[test]
    fn response_uses_sender_then_from() {
        let h = EmailHeaders {
            sender: Some("actual@sender.com"),
            from: "Marketing Team <news@company.com>",
            ..Default::default()
        };
        assert_eq!(response_email(&h), "actual@sender.com");
        let h = EmailHeaders {
            from: r#""John Doe" <john@example.com>"#,
            ..Default::default()
        };
        assert_eq!(response_email(&h), "john@example.com");
    }

    #[test]
    fn response_falls_back_to_return_path() {
        let h = EmailHeaders {
            from: "invalid",
            return_path: Some("<bounce@example.com>"),
            ..Default::default()
        };
        assert_eq!(response_email(&h), "bounce@example.com");
    }

    #[test]
    fn response_extracts_from_complex_formats() {
        let h = EmailHeaders {
            reply_to: Some(r#""Support Team" <support@company.com>"#),
            sender: Some("Marketing <news@company.com>"),
            from: "noreply@company.com",
            ..Default::default()
        };
        assert_eq!(response_email(&h), "support@company.com");
    }

    #[test]
    fn response_skips_invalid_candidates() {
        let h = EmailHeaders {
            reply_to: Some("invalid-email"),
            sender: Some("also-invalid"),
            from: "valid@example.com",
            ..Default::default()
        };
        assert_eq!(response_email(&h), "valid@example.com");
    }

    #[test]
    fn response_handles_encoded_names() {
        let h = EmailHeaders {
            from: "=?UTF-8?B?Sm9zw6k=?= <jose@test.com>",
            ..Default::default()
        };
        assert_eq!(response_email(&h), "jose@test.com");
    }

    #[test]
    fn response_all_invalid_returns_from() {
        assert_eq!(response_email(&EmailHeaders::default()), "");
        let h = EmailHeaders {
            from: "garbage",
            ..Default::default()
        };
        assert_eq!(response_email(&h), "garbage");
    }

    // ---- isBounceMessage ----

    #[test]
    fn bounce_detection() {
        for b in [
            "<>",
            "",
            "mailer-daemon@example.com",
            "MAILER-DAEMON@example.com",
            "postmaster@example.com",
            "bounce@mailer.com",
            "noreply@company.com",
            "no-reply@company.com",
            "donotreply@company.com",
            "do-not-reply@company.com",
            r#""Mailer Daemon" <mailer-daemon@example.com>"#,
        ] {
            assert!(is_bounce_message(b), "{b:?}");
        }
        for ok in ["user@example.com", "support@company.com"] {
            assert!(!is_bounce_message(ok), "{ok}");
        }
    }

    #[test]
    fn bounce_requires_exact_local_part() {
        // the TS prefix regex flagged "noreplyfan@x.com" and "bounce.team@x.com"
        assert!(!is_bounce_message("noreplyfan@example.com"));
        assert!(!is_bounce_message("postmaster-jane@example.com"));
        assert!(!is_bounce_message("just some text"));
    }

    // Rust has no null/undefined: the TS null/undefined cases are unrepresentable and not ported.

    proptest! {
        #[test]
        fn extract_never_panics(s in "\\PC*") {
            let _ = extract_email_address(&s);
            let _ = is_bounce_message(&s);
            let _ = normalize_email(&s);
        }

        #[test]
        fn extract_never_panics_on_bracket_soup(s in "[<>(),;:\"'\\\\@a-c.= ?]{0,60}") {
            let _ = extract_email_address(&s);
        }

        #[test]
        fn valid_output_is_a_fixed_point(s in "\\PC{0,80}") {
            if let Some(e) = extract_and_validate_email(&s) {
                prop_assert_eq!(extract_email_address(&e), e.clone());
                prop_assert!(!e.contains(char::is_whitespace));
            }
        }

        #[test]
        fn display_name_never_changes_the_address(
            name in "[A-Za-z ]{1,20}",
            local in "[a-z][a-z0-9.]{0,10}[a-z0-9]",
            domain in "[a-z]{2,10}\\.(com|org|io)",
        ) {
            prop_assume!(!local.contains(".."));
            let addr = format!("{local}@{domain}");
            for form in [
                format!("{name} <{addr}>"),
                format!("\"{name}\" <{addr}>"),
                format!("<{addr}>"),
                addr.clone(),
                format!("{name} ({name}) <{addr}>"),
            ] {
                prop_assert_eq!(extract_and_validate_email(&form), Some(addr.clone()), "{}", form);
            }
        }

        #[test]
        fn validator_rejects_anything_with_forbidden_chars(
            pre in "[a-z]{1,5}", bad in "[ \\t\\r\\n<>,;\"\\(\\)\\[\\]\\\\]", post in "[a-z]{1,5}"
        ) {
            let candidate = format!("{pre}{bad}{post}@example.com");
            prop_assert!(!is_valid_email(&candidate), "{:?}", candidate);
        }
    }
}
