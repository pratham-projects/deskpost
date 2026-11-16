use crate::testing::{FakeImap, FakeSmtp};
use crate::{
    Error, ImapConfig, ImapMailbox, Mailbox, OutboundEmail, ParseError, PollOptions, PollState,
    RawMessage, SmtpConfig, TlsMode, poll, send,
};

fn msg(n: u32) -> Vec<u8> {
    format!("From: a@example.com\r\nSubject: m{n}\r\nMessage-ID: <m{n}@x>\r\n\r\nbody {n}\r\n")
        .into_bytes()
}

/// In-memory mailbox that, like real servers, answers `UID n:*` with the top message even when
/// it is below n.
struct Fake {
    validity: Option<u32>,
    msgs: Vec<(u32, Vec<u8>)>,
    seen: Vec<u32>,
    fetched: Vec<Vec<u32>>,
}

impl Fake {
    fn with(uids: &[u32]) -> Self {
        Self {
            validity: Some(7),
            msgs: uids.iter().map(|u| (*u, msg(*u))).collect(),
            seen: vec![],
            fetched: vec![],
        }
    }
}

impl Mailbox for Fake {
    fn uid_validity(&self) -> Option<u32> {
        self.validity
    }
    async fn uids_after(&mut self, last_uid: u32) -> Result<Vec<u32>, Error> {
        let top = self.msgs.iter().map(|m| m.0).max();
        Ok(self
            .msgs
            .iter()
            .map(|m| m.0)
            .filter(|u| *u > last_uid || Some(*u) == top)
            .collect())
    }
    async fn fetch(&mut self, uids: &[u32]) -> Result<Vec<RawMessage>, Error> {
        self.fetched.push(uids.to_vec());
        Ok(self
            .msgs
            .iter()
            .filter(|m| uids.contains(&m.0))
            .map(|m| RawMessage {
                uid: m.0,
                bytes: m.1.clone(),
                oversize: None,
            })
            .collect())
    }
    async fn mark_seen(&mut self, uids: &[u32]) -> Result<(), Error> {
        self.seen.extend_from_slice(uids);
        Ok(())
    }
}

fn opts(batch: usize, mark_seen: bool) -> PollOptions {
    PollOptions {
        batch_size: batch,
        mark_seen,
        ..Default::default()
    }
}

#[tokio::test]
async fn poll_returns_only_newer_uids_and_advances_state() {
    let mut mb = Fake::with(&[1, 2, 5]);
    let out = poll(
        &mut mb,
        PollState {
            uid_validity: Some(7),
            last_uid: 2,
        },
        &opts(50, false),
    )
    .await
    .unwrap();
    let uids: Vec<_> = out.emails.iter().map(|p| p.uid).collect();
    assert_eq!(uids, [5]);
    assert_eq!(
        out.state,
        PollState {
            uid_validity: Some(7),
            last_uid: 5
        }
    );
    assert!(!out.has_more && !out.validity_reset);
    assert_eq!(out.emails[0].email.as_ref().unwrap().message_id, "m5@x");
}

#[tokio::test]
async fn poll_with_nothing_new_ignores_the_top_message_quirk() {
    let mut mb = Fake::with(&[1, 2, 3]);
    let out = poll(
        &mut mb,
        PollState {
            uid_validity: Some(7),
            last_uid: 3,
        },
        &opts(50, true),
    )
    .await
    .unwrap();
    assert!(out.emails.is_empty());
    assert_eq!(out.state.last_uid, 3);
    assert!(mb.fetched.is_empty() && mb.seen.is_empty());
}

#[tokio::test]
async fn poll_is_bounded_by_batch_size() {
    let mut mb = Fake::with(&(1..=7).collect::<Vec<_>>());
    let mut state = PollState::default();
    let mut seen_uids = vec![];
    let mut rounds = 0;
    loop {
        let out = poll(&mut mb, state, &opts(3, false)).await.unwrap();
        assert!(out.emails.len() <= 3);
        seen_uids.extend(out.emails.iter().map(|p| p.uid));
        state = out.state;
        rounds += 1;
        if !out.has_more {
            break;
        }
    }
    assert_eq!(seen_uids, (1..=7).collect::<Vec<_>>());
    assert_eq!(rounds, 3);
    assert!(mb.fetched.iter().all(|b| b.len() <= 3));
}

#[tokio::test]
async fn poll_marks_seen_only_when_asked() {
    let mut mb = Fake::with(&[1, 2]);
    poll(&mut mb, PollState::default(), &opts(50, false))
        .await
        .unwrap();
    assert!(mb.seen.is_empty());
    poll(&mut mb, PollState::default(), &opts(50, true))
        .await
        .unwrap();
    assert_eq!(mb.seen, [1, 2]);
}

#[tokio::test]
async fn uid_validity_change_resets_last_uid() {
    let mut mb = Fake::with(&[1, 2]);
    mb.validity = Some(8);
    let out = poll(
        &mut mb,
        PollState {
            uid_validity: Some(7),
            last_uid: 40,
        },
        &opts(50, false),
    )
    .await
    .unwrap();
    assert!(out.validity_reset);
    assert_eq!(out.emails.len(), 2);
    assert_eq!(
        out.state,
        PollState {
            uid_validity: Some(8),
            last_uid: 2
        }
    );
}

#[tokio::test]
async fn first_poll_records_validity_without_reset() {
    let mut mb = Fake::with(&[1, 2, 3]);
    let out = poll(
        &mut mb,
        PollState {
            uid_validity: None,
            last_uid: 2,
        },
        &opts(50, false),
    )
    .await
    .unwrap();
    assert!(!out.validity_reset);
    assert_eq!(out.emails.iter().map(|p| p.uid).collect::<Vec<_>>(), [3]);
    assert_eq!(out.state.uid_validity, Some(7));
}

#[tokio::test]
async fn unparseable_message_is_reported_and_consumed() {
    let mut mb = Fake::with(&[1, 2]);
    mb.msgs[0].1 = b"\x00\x01 garbage".to_vec();
    let out = poll(&mut mb, PollState::default(), &opts(50, false))
        .await
        .unwrap();
    assert_eq!(
        out.emails[0].email.as_ref().unwrap_err(),
        &ParseError::Unparseable
    );
    assert!(out.emails[1].email.is_ok());
    assert_eq!(out.state.last_uid, 2);
}

// ---- real ImapMailbox against the in-process server ----

fn cfg(port: u16, pass: &str) -> ImapConfig {
    let mut c = ImapConfig::new("127.0.0.1", port, TlsMode::None, "desk", pass);
    c.allow_private = true;
    c
}

#[tokio::test]
async fn imap_poller_end_to_end() {
    let server = FakeImap::start("desk", "pw").await;
    for n in 1..=4 {
        server.push(&msg(n));
    }
    let mut mb = ImapMailbox::connect(&cfg(server.port(), "pw"))
        .await
        .unwrap();
    assert_eq!(mb.uid_validity(), Some(1));

    let out = poll(&mut mb, PollState::default(), &opts(3, true))
        .await
        .unwrap();
    assert_eq!(
        out.emails.iter().map(|p| p.uid).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert!(out.has_more);
    assert_eq!(out.emails[0].email.as_ref().unwrap().subject, "m1");
    assert!(server.is_seen(1) && server.is_seen(3) && !server.is_seen(4));

    let out = poll(&mut mb, out.state, &opts(3, false)).await.unwrap();
    assert_eq!(out.emails.iter().map(|p| p.uid).collect::<Vec<_>>(), [4]);
    assert!(!server.is_seen(4), "BODY.PEEK must not set \\Seen");

    // nothing new: the `UID 5:*` top-message quirk must not resurrect uid 4
    let out = poll(&mut mb, out.state, &opts(3, false)).await.unwrap();
    assert!(out.emails.is_empty());

    // mailbox rebuilt on the server
    let late = server.push(&msg(9));
    assert_eq!(late, 5);
    server.reset_mailbox(2);
    server.push(&msg(10));
    mb.logout().await.unwrap();
    let mut mb = ImapMailbox::connect(&cfg(server.port(), "pw"))
        .await
        .unwrap();
    let out = poll(&mut mb, out.state, &opts(3, false)).await.unwrap();
    assert!(out.validity_reset);
    assert_eq!(out.emails.len(), 1);
    assert_eq!(
        out.state,
        PollState {
            uid_validity: Some(2),
            last_uid: 1
        }
    );
}

#[tokio::test]
async fn imap_bad_password_is_an_error() {
    let server = FakeImap::start("desk", "pw").await;
    let err = ImapMailbox::connect(&cfg(server.port(), "nope"))
        .await
        .err()
        .expect("must fail");
    assert!(matches!(err, Error::Imap(_)), "{err:?}");
}

#[tokio::test]
async fn smtp_to_imap_loop_through_fakes_keeps_threading() {
    // Outbound mail captured by the SMTP sink, delivered to the IMAP fake, polled back.
    let smtp = FakeSmtp::start().await;
    let imap = FakeImap::start("desk", "pw").await;
    let mut c = SmtpConfig::new(
        "127.0.0.1",
        smtp.port(),
        TlsMode::None,
        "support@example.com",
    );
    c.allow_private = true;
    let id = send(
        &c,
        &OutboundEmail {
            to: vec!["cust@example.org".into()],
            subject: "Re: Help".into(),
            text: Some("hello".into()),
            in_reply_to: Some("<orig@cust.example>".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    imap.push(&smtp.messages()[0].data);
    let mut mb = ImapMailbox::connect(&cfg(imap.port(), "pw")).await.unwrap();
    let out = poll(&mut mb, PollState::default(), &opts(50, false))
        .await
        .unwrap();
    let e = out.emails[0].email.as_ref().unwrap();
    assert_eq!(format!("<{}>", e.message_id), id);
    assert_eq!(e.in_reply_to.as_deref(), Some("orig@cust.example"));
}

/// Needs a GreenMail standalone server; skipped unless run with `GREENMAIL=1 cargo test -p
/// dp-mail -- --ignored`.
///
/// Setup (jar from Maven Central into the gitignored `.tools/`):
///   mkdir -p .tools && curl -L -o .tools/greenmail.jar \
///     https://repo1.maven.org/maven2/com/icegreen/greenmail-standalone/2.1.3/greenmail-standalone-2.1.3.jar
///   java -Dgreenmail.setup.test.smtp -Dgreenmail.setup.test.imap \
///     -Dgreenmail.hostname=127.0.0.1 -Dgreenmail.users=desk:pw@example.com \
///     -jar .tools/greenmail.jar
/// GreenMail listens on SMTP 3025 and IMAP 3143 (plaintext).
#[tokio::test]
#[ignore = "needs GreenMail on localhost:3025/3143 and GREENMAIL=1"]
async fn greenmail_round_trip() {
    if std::env::var("GREENMAIL").as_deref() != Ok("1") {
        eprintln!("GREENMAIL!=1, skipping");
        return;
    }
    let mut smtp = SmtpConfig::new("127.0.0.1", 3025, TlsMode::None, "support@example.com");
    smtp.allow_private = true;
    let id = send(
        &smtp,
        &OutboundEmail {
            to: vec!["desk@example.com".into()],
            subject: "greenmail round trip".into(),
            text: Some("hello".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut icfg = ImapConfig::new("127.0.0.1", 3143, TlsMode::None, "desk", "pw");
    icfg.allow_private = true;
    let mut mb = ImapMailbox::connect(&icfg).await.unwrap();
    let out = poll(&mut mb, PollState::default(), &opts(50, true))
        .await
        .unwrap();
    assert!(out.emails.iter().any(|p| {
        p.email
            .as_ref()
            .is_ok_and(|e| format!("<{}>", e.message_id) == id)
    }));
}

// ---- SSRF guard and hang protection ----

#[tokio::test]
async fn imap_refuses_internal_addresses_by_default() {
    let server = FakeImap::start("desk", "pw").await;
    let mut c = cfg(server.port(), "pw");
    c.allow_private = false;
    let e = ImapMailbox::connect(&c).await.err().expect("must refuse");
    assert!(e.to_string().contains("internal"), "{e}");
}

#[tokio::test]
async fn smtp_refuses_internal_addresses_by_default() {
    let smtp = FakeSmtp::start().await;
    let c = SmtpConfig::new("127.0.0.1", smtp.port(), TlsMode::None, "a@example.com");
    let e = send(
        &c,
        &OutboundEmail {
            to: vec!["cust@example.org".into()],
            subject: "x".into(),
            text: Some("x".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(e.to_string().contains("internal"), "{e}");
}

#[tokio::test]
async fn a_server_that_never_greets_times_out() {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let _held = tokio::spawn(async move {
        let _conn = l.accept().await;
        std::future::pending::<()>().await;
    });
    let mut c = cfg(port, "pw");
    c.io_timeout = std::time::Duration::from_millis(200);
    let started = std::time::Instant::now();
    let e = ImapMailbox::connect(&c).await.err().expect("must time out");
    assert!(matches!(e, Error::Timeout(_)), "{e}");
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[tokio::test]
async fn oversize_mail_is_skipped_without_download_and_consumed() {
    let server = FakeImap::start("desk", "pw").await;
    server.push(&vec![b'x'; 5000]);
    let uid_ok =
        server.push(b"From: a@example.org\r\nSubject: ok\r\nMessage-ID: <1@x>\r\n\r\nhi\r\n");
    let mut c = cfg(server.port(), "pw");
    c.max_message_bytes = 1000;
    let mut mb = ImapMailbox::connect(&c).await.unwrap();
    let mut o = opts(50, false);
    o.limits.max_message_bytes = 1000;
    let out = poll(&mut mb, PollState::default(), &o).await.unwrap();
    assert!(matches!(
        out.emails[0].email,
        Err(ParseError::MessageTooLarge { size: 5000, .. })
    ));
    assert!(out.emails[1].email.is_ok());
    assert_eq!(out.emails[1].uid, uid_ok);
    assert_eq!(
        out.state.last_uid, uid_ok,
        "cursor moves past the oversize mail"
    );
}
