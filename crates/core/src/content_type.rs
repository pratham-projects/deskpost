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

/// True when the bytes start like a native executable or script: Windows PE (`MZ`), ELF, Mach-O
/// (thin and fat), or a `#!` interpreter line. The declared type of an upload is the client's word;
/// this is the check on the content, so a program cannot ride in as `application/pdf`.
pub fn looks_executable(bytes: &[u8]) -> bool {
    const MAGIC: [&[u8]; 8] = [
        b"MZ",
        b"\x7fELF",
        b"#!",
        &[0xFE, 0xED, 0xFA, 0xCE],
        &[0xFE, 0xED, 0xFA, 0xCF],
        &[0xCE, 0xFA, 0xED, 0xFE],
        &[0xCF, 0xFA, 0xED, 0xFE],
        &[0xCA, 0xFE, 0xBA, 0xBE],
    ];
    MAGIC.iter().any(|m| bytes.starts_with(m))
}

/// Media type implied by a file name's extension, for clients that send `application/octet-stream`
/// or no type. Only types an upload may have; anything else is `None`.
pub fn type_from_extension(filename: &str) -> Option<&'static str> {
    let ext = filename.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "pdf" => "application/pdf",
        "txt" | "log" => "text/plain",
        "csv" => "text/csv",
        "zip" => "application/zip",
        "doc" => "application/msword",
        "xls" => "application/vnd.ms-excel",
        "ppt" => "application/vnd.ms-powerpoint",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        _ => return None,
    })
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

    #[test]
    fn executables_are_recognised_by_content() {
        assert!(looks_executable(b"MZ\x90\x00"));
        assert!(looks_executable(b"\x7fELF\x02"));
        assert!(looks_executable(b"#!/bin/sh\n"));
        assert!(looks_executable(&[0xCF, 0xFA, 0xED, 0xFE, 0x07]));
        assert!(!looks_executable(b"%PDF-1.7"));
        assert!(!looks_executable(b"PK\x03\x04"));
        assert!(!looks_executable(b"M"));
        assert!(!looks_executable(b""));
    }

    #[test]
    fn extension_maps_to_an_allowed_type_only() {
        assert_eq!(type_from_extension("a.PDF"), Some("application/pdf"));
        assert_eq!(type_from_extension("x.tar.zip"), Some("application/zip"));
        assert_eq!(type_from_extension("evil.exe"), None);
        assert_eq!(type_from_extension("noext"), None);
        assert_eq!(type_from_extension("icon.svg"), None);
    }
}
