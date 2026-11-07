const MAX_NAME_BYTES: usize = 255;

fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}')
}

/// Metadata-only display name: no path parts, control/bidi chars or header-hostile chars.
/// Never empty. Not safe to use as a path component for that reason: it is never used as one.
pub fn sanitize_filename(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_control() || is_bidi_control(c) || "\"<>:|?*;".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut name = cleaned
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .to_string();
    while name.len() > MAX_NAME_BYTES {
        name.pop();
    }
    // Truncation can leave a trailing dot or space (`name.` is a different name on Windows).
    name.truncate(
        name.trim_end_matches(|c: char| c == '.' || c.is_whitespace())
            .len(),
    );
    if name.is_empty() { "file".into() } else { name }
}

/// `type/subtype` made of token characters, else `application/octet-stream`.
pub fn sanitize_content_type(raw: &str) -> String {
    let base = raw
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let tok = |s: &str| {
        !s.is_empty()
            && s.len() <= 127
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
    };
    match base.split_once('/') {
        Some((t, s)) if tok(t) && tok(s) => base,
        _ => "application/octet-stream".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn strips_paths_and_junk() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("C:\\x\\evil.exe"), "evil.exe");
        assert_eq!(sanitize_filename("a\0b\r\n.txt"), "a_b__.txt");
        assert_eq!(sanitize_filename("re\u{202E}gnp.exe"), "re_gnp.exe");
        assert_eq!(sanitize_filename("..."), "file");
        assert_eq!(sanitize_filename("x\"; filename=\"y"), "x__ filename=_y");
        assert_eq!(sanitize_filename("a\";b.txt"), "a__b.txt");
        assert_eq!(sanitize_filename(".hidden"), "hidden");
    }

    #[test]
    fn truncates_on_char_boundary() {
        let long = "é".repeat(300);
        let out = sanitize_filename(&long);
        assert!(out.len() <= 255);
        assert!(out.chars().all(|c| c == 'é'));
    }

    #[test]
    fn content_type() {
        assert_eq!(sanitize_content_type("Image/PNG; charset=x"), "image/png");
        assert_eq!(
            sanitize_content_type("text/html\r\nX: y"),
            "application/octet-stream"
        );
        assert_eq!(sanitize_content_type(""), "application/octet-stream");
        assert_eq!(sanitize_content_type("nope"), "application/octet-stream");
    }

    proptest! {
        #[test]
        fn filename_always_clean(raw in ".*") {
            let n = sanitize_filename(&raw);
            prop_assert!(!n.is_empty() && n.len() <= 255);
            prop_assert!(!n.contains(['/', '\\', '\0', '"']));
            prop_assert!(!n.chars().any(|c| c.is_control() || is_bidi_control(c)));
            prop_assert!(!n.starts_with('.'));
        }
    }

    #[test]
    fn truncation_leaves_no_trailing_dot_or_space() {
        let raw = format!("{} .x", "a".repeat(252));
        let out = sanitize_filename(&raw);
        assert!(out.len() <= 255);
        assert!(!out.ends_with(['.', ' ']), "{out:?}");
    }
}
