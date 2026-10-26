//! Attachment content-type classification. Only a small allowlist of passive types may be
//! displayed inline; everything else is served as a download with `application/octet-stream`.
//! SVG, HTML, XML and PDF are deliberately download-only (script-capable when rendered).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Inline,
    Download,
}

const INLINE_SAFE: [&str; 8] = [
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/avif",
    "image/bmp",
    "text/plain",
    "text/csv",
];

/// Lowercased `type/subtype` without parameters, or `None` if it is not a well-formed media type.
pub fn normalize(content_type: &str) -> Option<String> {
    let essence = content_type.split(';').next()?.trim().to_ascii_lowercase();
    let (ty, sub) = essence.split_once('/')?;
    let token = |s: &str| {
        !s.is_empty()
            && s.len() <= 100
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
    };
    (token(ty) && token(sub)).then_some(essence)
}

pub fn classify(content_type: &str) -> Disposition {
    match normalize(content_type) {
        Some(ct) if INLINE_SAFE.contains(&ct.as_str()) => Disposition::Inline,
        _ => Disposition::Download,
    }
}

/// The `Content-Type` to serve: the normalised allowlisted type, else `application/octet-stream`.
/// Download-only types are never echoed back, so a hostile type cannot reach a response header.
pub fn served_content_type(content_type: &str) -> &'static str {
    match normalize(content_type) {
        Some(ct) => INLINE_SAFE
            .iter()
            .find(|t| **t == ct)
            .copied()
            .unwrap_or("application/octet-stream"),
        None => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn safe_images_and_text_are_inline() {
        for ct in [
            "image/png",
            "image/jpeg",
            "IMAGE/GIF",
            "image/webp",
            "text/plain",
        ] {
            assert_eq!(classify(ct), Disposition::Inline, "{ct}");
        }
    }

    #[test]
    fn parameters_and_whitespace_are_ignored() {
        assert_eq!(classify(" text/plain; charset=utf-8 "), Disposition::Inline);
        assert_eq!(classify("image/png;foo=bar"), Disposition::Inline);
    }

    #[test]
    fn active_content_is_download_only() {
        for ct in [
            "image/svg+xml",
            "text/html",
            "application/xhtml+xml",
            "text/xml",
            "application/xml",
            "application/pdf",
            "application/javascript",
            "text/javascript",
            "application/x-msdownload",
            "application/octet-stream",
        ] {
            assert_eq!(classify(ct), Disposition::Download, "{ct}");
            assert_eq!(served_content_type(ct), "application/octet-stream", "{ct}");
        }
    }

    #[test]
    fn malformed_types_are_download_only() {
        for ct in [
            "",
            "png",
            "image/",
            "/png",
            "image/png\r\nX: y",
            "text/plain, text/html",
            "a b/c",
        ] {
            assert_eq!(classify(ct), Disposition::Download, "{ct:?}");
            assert_eq!(served_content_type(ct), "application/octet-stream");
        }
    }

    #[test]
    fn served_type_is_canonical() {
        assert_eq!(served_content_type("Image/PNG; x=1"), "image/png");
    }

    proptest! {
        #[test]
        fn served_type_is_always_safe(ct in ".*") {
            let served = served_content_type(&ct);
            prop_assert!(served == "application/octet-stream" || INLINE_SAFE.contains(&served));
            prop_assert_eq!(
                classify(&ct) == Disposition::Inline,
                served != "application/octet-stream"
            );
        }
    }
}
