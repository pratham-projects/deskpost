use proptest::prelude::*;

use crate::{Limits, ParseError, parse_inbound};

fn parse(raw: &str) -> crate::InboundEmail {
    parse_inbound(raw.as_bytes(), &Limits::default()).unwrap()
}

const SIMPLE: &str = "From: Ann <ann@example.com>\r\nTo: Desk <desk@example.org>, b@example.org\r\n\
Subject: Hello\r\nMessage-ID: <abc@example.com>\r\nIn-Reply-To: <p@example.org>\r\n\
References: <r1@example.org> <p@example.org>\r\nDate: Tue, 1 Jul 2025 10:00:00 +0000\r\n\r\nHi there\r\n";

#[test]
fn simple_message_fields() {
    let e = parse(SIMPLE);
    assert_eq!(e.message_id, "abc@example.com");
    assert_eq!(e.in_reply_to.as_deref(), Some("p@example.org"));
    assert_eq!(e.references, ["r1@example.org", "p@example.org"]);
    assert_eq!(e.from.as_deref(), Some("ann@example.com"));
    assert_eq!(e.from_name.as_deref(), Some("Ann"));
    assert_eq!(e.to, ["desk@example.org", "b@example.org"]);
    assert_eq!(e.subject, "Hello");
    assert_eq!(e.text.as_deref().map(str::trim), Some("Hi there"));
    assert_eq!(e.html, None);
    assert_eq!(e.date, Some(1_751_364_000));
    assert!(e.attachments.is_empty());
}

#[test]
fn machine_generated_mail_is_flagged() {
    let with = |h: &str| {
        parse(&format!(
            "From: a@example.com\r\n{h}\r\nSubject: x\r\n\r\nbody\r\n"
        ))
    };
    assert!(!with("X-Other: 1").auto_generated);
    assert!(!with("Auto-Submitted: no").auto_generated);
    assert!(!with("Precedence: first-class").auto_generated);
    assert!(with("Auto-Submitted: auto-replied").auto_generated);
    assert!(with("auto-submitted: Auto-Generated").auto_generated);
    assert!(with("Precedence: Bulk").auto_generated);
    assert!(with("Precedence: list").auto_generated);
    assert!(with("X-Autoreply: yes").auto_generated);
}

#[test]
fn missing_message_id_is_derived_and_stable() {
    let raw = SIMPLE.replace("Message-ID: <abc@example.com>\r\n", "");
    let (a, b) = (parse(&raw), parse(&raw));
    assert_eq!(a.message_id, b.message_id);
    assert!(
        a.message_id.ends_with("@generated.invalid") && a.message_id.len() > 20,
        "{}",
        a.message_id
    );
    let other = parse(&raw.replace("Hi there", "Different"));
    assert_ne!(a.message_id, other.message_id);
}

#[test]
fn nested_multipart_with_inline_image_and_attachment() {
    let raw = "From: a@example.com\r\nTo: b@example.org\r\nSubject: Nested\r\nMessage-ID: <n@x>\r\n\
MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"outer\"\r\n\r\n\
--outer\r\nContent-Type: multipart/related; boundary=\"rel\"\r\n\r\n\
--rel\r\nContent-Type: multipart/alternative; boundary=\"alt\"\r\n\r\n\
--alt\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nplain part\r\n\
--alt\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>html <img src=\"cid:logo\"></p>\r\n--alt--\r\n\
--rel\r\nContent-Type: image/png; name=\"logo.png\"\r\nContent-ID: <logo>\r\nContent-Disposition: inline; filename=\"logo.png\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\niVBORw0KGgo=\r\n--rel--\r\n\
--outer\r\nContent-Type: application/pdf; name=\"doc.pdf\"\r\nContent-Disposition: attachment; filename=\"doc.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\nSGVsbG8gUERG\r\n--outer--\r\n";
    let e = parse(raw);
    assert_eq!(e.text.as_deref().map(str::trim), Some("plain part"));
    assert!(e.html.as_deref().unwrap().contains("cid:logo"));
    let img = e
        .attachments
        .iter()
        .find(|a| a.filename == "logo.png")
        .expect("inline image");
    assert!(img.inline);
    assert_eq!(img.content_id.as_deref(), Some("logo"));
    assert_eq!(img.content_type, "image/png");
    assert_eq!(img.bytes, b"\x89PNG\r\n\x1a\n");
    let pdf = e
        .attachments
        .iter()
        .find(|a| a.filename == "doc.pdf")
        .expect("pdf");
    assert!(!pdf.inline);
    assert_eq!(pdf.bytes, b"Hello PDF");
}

#[test]
fn charsets_decode_and_broken_ones_do_not_fail() {
    let latin = b"From: a@example.com\r\nSubject: =?ISO-8859-1?Q?caf=E9?=\r\nMessage-ID: <c@x>\r\n\
Content-Type: text/plain; charset=iso-8859-1\r\nContent-Transfer-Encoding: 8bit\r\n\r\ncaf\xe9 cr\xe8me\r\n";
    let e = parse_inbound(latin, &Limits::default()).unwrap();
    assert_eq!(e.subject, "café");
    assert_eq!(e.text.as_deref().map(str::trim), Some("café crème"));

    let bogus = b"From: a@example.com\r\nSubject: x\r\nMessage-ID: <d@x>\r\n\
Content-Type: text/plain; charset=totally-bogus-9\r\n\r\nplain ascii \xff\xfe bytes\r\n";
    let e = parse_inbound(bogus, &Limits::default()).unwrap();
    assert!(e.text.unwrap().contains("plain ascii"));

    let bad_utf8 = b"From: a@example.com\r\nMessage-ID: <e@x>\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nok \xc3\x28 end\r\n";
    let e = parse_inbound(bad_utf8, &Limits::default()).unwrap();
    assert!(e.text.unwrap().contains("end"));
}

#[test]
fn size_caps_return_typed_errors() {
    let small = Limits {
        max_message_bytes: 100,
        ..Limits::default()
    };
    assert!(matches!(
        parse_inbound(SIMPLE.as_bytes(), &small),
        Err(ParseError::MessageTooLarge { limit: 100, .. })
    ));

    let part = |n: usize| {
        format!(
            "--b\r\nContent-Type: text/plain; name=\"f{n}.txt\"\r\nContent-Disposition: attachment; filename=\"f{n}.txt\"\r\n\r\n{}\r\n",
            "x".repeat(50)
        )
    };
    let mail = |n: usize| {
        let mut s = String::from(
            "From: a@example.com\r\nMessage-ID: <m@x>\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nbody\r\n",
        );
        for i in 0..n {
            s.push_str(&part(i));
        }
        s.push_str("--b--\r\n");
        s
    };
    let l = Limits {
        max_attachments: 2,
        ..Limits::default()
    };
    assert_eq!(
        parse_inbound(mail(2).as_bytes(), &l)
            .unwrap()
            .attachments
            .len(),
        2
    );
    assert_eq!(
        parse_inbound(mail(3).as_bytes(), &l),
        Err(ParseError::TooManyAttachments { limit: 2 })
    );

    let l = Limits {
        max_attachment_bytes: 10,
        ..Limits::default()
    };
    assert!(matches!(
        parse_inbound(mail(1).as_bytes(), &l),
        Err(ParseError::AttachmentTooLarge { limit: 10, ref filename, .. }) if filename == "f0.txt"
    ));

    let l = Limits {
        max_headers: 3,
        ..Limits::default()
    };
    assert!(matches!(
        parse_inbound(SIMPLE.as_bytes(), &l),
        Err(ParseError::TooManyHeaders { limit: 3, .. })
    ));
}

#[test]
fn garbage_is_unparseable() {
    for raw in [&b""[..], b"\x00\x01\x02", b"no headers at all, just text"] {
        assert_eq!(
            parse_inbound(raw, &Limits::default()),
            Err(ParseError::Unparseable),
            "{raw:?}"
        );
    }
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(raw in prop::collection::vec(any::<u8>(), 0..2048)) {
        let _ = parse_inbound(&raw, &Limits::default());
    }

    #[test]
    fn derived_id_is_deterministic(body in "[a-z ]{1,60}") {
        let raw = format!("From: a@example.com\r\nSubject: s\r\n\r\n{body}\r\n");
        prop_assert_eq!(parse(&raw).message_id, parse(&raw).message_id);
    }
}
