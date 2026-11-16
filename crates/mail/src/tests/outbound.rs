use std::time::Duration;

use proptest::prelude::*;

use crate::testing::FakeSmtp;
use crate::{
    Error, Limits, OutAttachment, OutboundEmail, SmtpConfig, TlsMode, build_message, parse_inbound,
    send,
};

fn cfg(port: u16) -> SmtpConfig {
    let mut c = SmtpConfig::new("127.0.0.1", port, TlsMode::None, "support@example.com");
    c.from_name = Some("Support Desk".into());
    c.allow_private = true;
    c
}

const EVIL: &str = "x\r\nBcc: evil@example.com";

type Mutate = Box<dyn Fn(&mut SmtpConfig, &mut OutboundEmail)>;

fn mail() -> OutboundEmail {
    OutboundEmail {
        to: vec!["cust@example.org".into()],
        subject: "Re: Help".into(),
        text: Some("plain body".into()),
        html: Some("<p>html body</p>".into()),
        ..Default::default()
    }
}

fn formatted(c: &SmtpConfig, m: &OutboundEmail) -> String {
    String::from_utf8_lossy(&build_message(c, m).unwrap().0.formatted()).into_owned()
}

#[test]
fn rejects_cr_lf_in_every_header_value() {
    let cases: Vec<(&str, Mutate)> = vec![
        ("subject", Box::new(|_, m| m.subject = EVIL.into())),
        (
            "to",
            Box::new(|_, m| m.to = vec![format!("a@example.org{EVIL}")]),
        ),
        (
            "cc",
            Box::new(|_, m| m.cc = vec![format!("a@example.org{EVIL}")]),
        ),
        (
            "bcc",
            Box::new(|_, m| m.bcc = vec![format!("a@example.org{EVIL}")]),
        ),
        (
            "from name",
            Box::new(|c, _| c.from_name = Some(EVIL.into())),
        ),
        (
            "from address",
            Box::new(|c, _| c.from_address = format!("a@example.org{EVIL}")),
        ),
        (
            "message id",
            Box::new(|_, m| m.message_id = Some(EVIL.into())),
        ),
        (
            "in reply to",
            Box::new(|_, m| m.in_reply_to = Some(EVIL.into())),
        ),
        (
            "references",
            Box::new(|_, m| m.references = vec![EVIL.into()]),
        ),
        (
            "custom value",
            Box::new(|_, m| m.headers = vec![("X-Ok".into(), EVIL.into())]),
        ),
        (
            "custom name",
            Box::new(|_, m| m.headers = vec![(format!("X-A{EVIL}"), "v".into())]),
        ),
        ("bare LF", Box::new(|_, m| m.subject = "a\nb".into())),
        ("bare CR", Box::new(|_, m| m.subject = "a\rb".into())),
        (
            "attachment name",
            Box::new(|_, m| {
                m.attachments = vec![OutAttachment {
                    filename: EVIL.into(),
                    content_type: "text/plain".into(),
                    bytes: vec![1],
                }]
            }),
        ),
        (
            "attachment type",
            Box::new(|_, m| {
                m.attachments = vec![OutAttachment {
                    filename: "a.txt".into(),
                    content_type: format!("text/plain{EVIL}"),
                    bytes: vec![1],
                }]
            }),
        ),
    ];
    for (label, mutate) in cases {
        let (mut c, mut m) = (cfg(1), mail());
        mutate(&mut c, &mut m);
        assert!(build_message(&c, &m).is_err(), "{label} accepted CR/LF");
    }
}

#[test]
fn custom_header_rules() {
    let (c, mut m) = (cfg(1), mail());
    for bad in [
        "Subject",
        "from",
        "Message-ID",
        "Content-Type",
        "",
        "X Bad",
        "X:Bad",
    ] {
        m.headers = vec![(bad.into(), "v".into())];
        assert!(
            build_message(&c, &m).is_err(),
            "header name {bad:?} accepted"
        );
    }
    m.headers = vec![("X-Note".into(), "caf\u{e9}".into())];
    assert!(build_message(&c, &m).is_err(), "non-ascii value accepted");
    m.headers = vec![
        ("X-Ticket-Id".into(), "T-42".into()),
        ("List-Unsubscribe".into(), "<mailto:u@example.com>".into()),
    ];
    let out = formatted(&c, &m);
    assert!(out.contains("X-Ticket-Id: T-42\r\n"), "{out}");
    assert!(out.contains("List-Unsubscribe: <mailto:u@example.com>\r\n"));
}

#[test]
fn threading_headers_and_alternatives() {
    let c = cfg(1);
    let mut m = mail();
    m.in_reply_to = Some("parent@mail.example".into());
    m.references = vec!["<root@mail.example>".into(), "parent@mail.example".into()];
    m.message_id = Some("<mine@desk.example>".into());
    let out = formatted(&c, &m);
    assert!(out.contains("Message-ID: <mine@desk.example>\r\n"), "{out}");
    assert!(out.contains("In-Reply-To: <parent@mail.example>\r\n"));
    assert!(out.contains("References: <root@mail.example> <parent@mail.example>\r\n"));
    assert!(out.contains("multipart/alternative"));
    assert!(out.contains("text/plain") && out.contains("text/html"));
    assert!(
        out.contains("From: \"Support Desk\" <support@example.com>"),
        "{out}"
    );
}

#[test]
fn generated_message_ids_are_unique_and_use_sender_domain() {
    let (c, m) = (cfg(1), mail());
    let (_, a) = build_message(&c, &m).unwrap();
    let (_, b) = build_message(&c, &m).unwrap();
    assert_ne!(a, b);
    assert!(a.starts_with('<') && a.ends_with("@example.com>"), "{a}");
}

#[test]
fn body_and_recipient_validation() {
    let c = cfg(1);
    let mut m = mail();
    m.text = None;
    m.html = None;
    assert!(matches!(
        build_message(&c, &m),
        Err(Error::Invalid { field: "body", .. })
    ));
    let mut m = mail();
    m.to.clear();
    assert!(matches!(
        build_message(&c, &m),
        Err(Error::Invalid { field: "to", .. })
    ));
    let mut m = mail();
    m.to = vec!["not-an-address".into()];
    assert!(build_message(&c, &m).is_err());
    // single-body variants still build
    let mut m = mail();
    m.html = None;
    assert!(formatted(&c, &m).contains("text/plain"));
    let mut m = mail();
    m.text = None;
    assert!(formatted(&c, &m).contains("text/html"));
}

#[tokio::test]
async fn send_round_trip_through_fake_smtp() {
    let smtp = FakeSmtp::start().await;
    let mut c = cfg(smtp.port());
    c.username = Some("user".into());
    c.password = Some("p@ss word".into());
    let mut m = mail();
    m.cc = vec!["cc@example.org".into()];
    m.bcc = vec!["hidden@example.org".into()];
    m.in_reply_to = Some("<parent@mail.example>".into());
    m.references = vec!["<root@mail.example>".into()];
    m.headers = vec![("X-Ticket-Id".into(), "T-42".into())];
    m.attachments = vec![OutAttachment {
        filename: "report.pdf".into(),
        content_type: "application/pdf".into(),
        bytes: (0..=255u8).cycle().take(5000).collect(),
    }];
    let id = send(&c, &m).await.unwrap();

    let got = smtp.messages();
    assert_eq!(got.len(), 1);
    let g = &got[0];
    assert_eq!(g.mail_from, "support@example.com");
    assert_eq!(
        g.rcpt_to,
        ["cust@example.org", "cc@example.org", "hidden@example.org"]
    );
    assert_eq!(g.auth, Some(("user".into(), "p@ss word".into())));
    let raw = String::from_utf8_lossy(&g.data);
    assert!(
        !raw.contains("hidden@example.org"),
        "Bcc leaked into headers"
    );
    assert!(raw.contains("X-Ticket-Id: T-42"));

    let parsed = parse_inbound(&g.data, &Limits::default()).unwrap();
    assert_eq!(format!("<{}>", parsed.message_id), id);
    assert_eq!(parsed.in_reply_to.as_deref(), Some("parent@mail.example"));
    assert_eq!(parsed.references, ["root@mail.example"]);
    assert_eq!(parsed.from.as_deref(), Some("support@example.com"));
    assert_eq!(parsed.from_name.as_deref(), Some("Support Desk"));
    assert_eq!(parsed.subject, "Re: Help");
    assert_eq!(parsed.text.as_deref().map(str::trim), Some("plain body"));
    assert_eq!(
        parsed.html.as_deref().map(str::trim),
        Some("<p>html body</p>")
    );
    assert_eq!(parsed.attachments.len(), 1);
    assert_eq!(parsed.attachments[0].filename, "report.pdf");
    assert_eq!(parsed.attachments[0].content_type, "application/pdf");
    assert_eq!(
        parsed.attachments[0].bytes,
        (0..=255u8).cycle().take(5000).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn smtp_auth_failure_is_an_error() {
    let smtp = FakeSmtp::start_requiring_auth("user", "right").await;
    let mut c = cfg(smtp.port());
    c.username = Some("user".into());
    c.password = Some("wrong".into());
    assert!(matches!(send(&c, &mail()).await, Err(Error::Smtp(_))));
    assert!(smtp.messages().is_empty());
    c.password = Some("right".into());
    send(&c, &mail()).await.unwrap();
    assert_eq!(smtp.messages().len(), 1);
}

#[tokio::test]
async fn send_times_out_on_silent_server() {
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut c = cfg(silent.local_addr().unwrap().port());
    c.io_timeout = Duration::from_millis(200);
    c.total_timeout = Duration::from_millis(500);
    let started = std::time::Instant::now();
    let err = send(&c, &mail()).await.unwrap_err();
    assert!(matches!(err, Error::Smtp(_) | Error::Timeout(_)), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

proptest! {
    #[test]
    fn any_cr_or_lf_in_subject_is_rejected(pre in "[ -~]{0,20}", nl in prop::sample::select(vec!["\r\n", "\n", "\r"]), post in "[ -~]{0,20}") {
        let mut m = mail();
        m.subject = format!("{pre}{nl}Bcc: evil@example.com{post}");
        prop_assert!(build_message(&cfg(1), &m).is_err());
    }

    #[test]
    fn printable_subjects_never_add_header_lines(subject in "[ -~\u{a0}-\u{2fff}]{0,80}") {
        let mut m = mail();
        m.subject = subject;
        let out = formatted(&cfg(1), &m);
        let head = out.split("\r\n\r\n").next().unwrap();
        prop_assert!(!head.lines().any(|l| l.to_ascii_lowercase().starts_with("bcc:")));
    }
}

#[test]
fn config_debug_never_shows_passwords() {
    let mut c = cfg(25);
    c.password = Some("hunter2".into());
    let i = crate::ImapConfig::new("h", 143, TlsMode::None, "u", "hunter2");
    assert!(!format!("{c:?}{i:?}").contains("hunter2"));
}
