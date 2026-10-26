//! Attachment filename sanitiser. Output is a single safe path component: no separators,
//! control or bidi-override characters, never `.`/`..`, never a Windows reserved device name.

const MAX_BYTES: usize = 120;
const FALLBACK: &str = "attachment";
const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

fn is_bad(c: char) -> bool {
    c.is_control()
        || matches!(c, '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*')
        // bidi overrides/isolates, zero-width and BOM characters used for extension spoofing
        || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}')
}

pub fn sanitize_filename(name: &str) -> String {
    // Keep only the last path component, whichever separator the client used.
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .map(|c| if is_bad(c) { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim_matches(|c: char| c.is_whitespace() || c == '.');

    let (stem, ext) = match cleaned.rfind('.') {
        Some(i) if i > 0 => cleaned.split_at(i),
        _ => (cleaned, ""),
    };
    // Windows ignores the extension when matching device names: "nul.txt" is NUL.
    let stem = if RESERVED.contains(&stem.to_ascii_lowercase().as_str()) {
        format!("_{stem}")
    } else {
        stem.to_string()
    };
    let ext = truncate(ext, 20);
    let stem = truncate(&stem, MAX_BYTES - ext.len());
    let out = format!("{stem}{ext}");
    if out
        .trim_matches(|c: char| c == '_' || c.is_whitespace() || c == '.')
        .is_empty()
    {
        FALLBACK.to_string()
    } else {
        out
    }
}

fn truncate(s: &str, max: usize) -> &str {
    let mut end = max.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn plain_names_pass_through() {
        assert_eq!(sanitize_filename("report.pdf"), "report.pdf");
        assert_eq!(sanitize_filename("my photo (1).jpg"), "my photo (1).jpg");
        assert_eq!(sanitize_filename("日本語.txt"), "日本語.txt");
    }

    #[test]
    fn traversal_is_reduced_to_last_component() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("..\\..\\win\\system.ini"), "system.ini");
        assert_eq!(sanitize_filename("/abs/path/file.txt"), "file.txt");
        assert_eq!(sanitize_filename("a/b\\c.txt"), "c.txt");
    }

    #[test]
    fn dot_names_and_empty_fall_back() {
        for n in ["", ".", "..", "...", "  ", "/", "../", "\0", "___"] {
            assert_eq!(sanitize_filename(n), "attachment", "{n:?}");
        }
    }

    #[test]
    fn control_and_special_chars_are_replaced() {
        assert_eq!(sanitize_filename("a\0b\nc.txt"), "a_b_c.txt");
        assert_eq!(
            sanitize_filename("a:b*c?d\"e<f>g|h.txt"),
            "a_b_c_d_e_f_g_h.txt"
        );
    }

    #[test]
    fn bidi_override_cannot_spoof_extension() {
        let out = sanitize_filename("invoice\u{202E}fdp.exe");
        assert!(!out.contains('\u{202E}'));
        assert_eq!(out, "invoice_fdp.exe");
    }

    #[test]
    fn reserved_device_names_are_prefixed() {
        assert_eq!(sanitize_filename("CON"), "_CON");
        assert_eq!(sanitize_filename("nul.txt"), "_nul.txt");
        assert_eq!(sanitize_filename("com1.log"), "_com1.log");
        assert_eq!(sanitize_filename("console.txt"), "console.txt");
    }

    #[test]
    fn leading_dots_are_stripped_so_no_hidden_files() {
        assert_eq!(sanitize_filename(".htaccess"), "htaccess");
        assert_eq!(sanitize_filename("name. "), "name");
    }

    #[test]
    fn long_names_keep_extension_and_char_boundary() {
        let out = sanitize_filename(&format!("{}.pdf", "é".repeat(500)));
        assert!(out.len() <= MAX_BYTES);
        assert!(out.ends_with(".pdf"));
    }

    proptest! {
        #[test]
        fn output_is_always_a_safe_component(s in ".*") {
            let out = sanitize_filename(&s);
            prop_assert!(!out.is_empty());
            prop_assert!(out.len() <= MAX_BYTES);
            prop_assert!(!out.contains('/') && !out.contains('\\'));
            prop_assert!(!out.chars().any(|c| c.is_control()));
            prop_assert!(out != "." && out != "..");
            prop_assert!(!out.starts_with('.'));
        }

        #[test]
        fn sanitising_is_idempotent(s in ".*") {
            let once = sanitize_filename(&s);
            prop_assert_eq!(sanitize_filename(&once), once);
        }

        #[test]
        fn traversal_prefix_never_survives(s in "[a-z]{1,8}\\.[a-z]{1,3}", depth in 1usize..6) {
            let evil = format!("{}{s}", "../".repeat(depth));
            prop_assert_eq!(sanitize_filename(&evil), s);
        }
    }
}
