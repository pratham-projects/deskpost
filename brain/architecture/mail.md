---
type: Architecture
title: Mail Crate (sd-mail)
description: SMTP sender, MIME parser, IMAP poller and in-process fake servers.
tags: [mail, smtp, imap, testing]
timestamp: 2026-10-27
---

# Mail Crate (sd-mail)

Behaviour and free-provider settings: `docs/free-email.md`. Decision: [ADR-0001](/decisions/0001-free-email.md).

- **Outbound** (`outbound.rs`): `send(&SmtpConfig, &OutboundEmail) -> Message-ID`. lettre async, config per call, `TlsMode` StartTls/Implicit/None. Text+html alternatives, attachments from caller bytes, In-Reply-To/References/Message-ID, custom headers (ASCII, reserved names refused). Any control char in any header value is `Error::HeaderInjection`. Per-command `io_timeout` plus overall `total_timeout`.
- **Inbound** (`inbound.rs`): `parse_inbound(bytes, &Limits) -> InboundEmail` via mail-parser 1.x. Typed `ParseError` for message/attachment size, attachment count, header count. Missing Message-ID becomes a SHA-256 derived id. `html` is only a real text/html part (mail-parser otherwise synthesises one from text).
- **IMAP** (`imap.rs`): `Mailbox` trait (uid_validity, uids_after, fetch, mark_seen) with `ImapMailbox` over async-imap + tokio-rustls (webpki roots, ring). `poll()` does one bounded batch (default 50) and returns the next `PollState`; interval and looping belong to the caller (R23 worker). UIDVALIDITY change resets `last_uid` to 0. Fetch uses `BODY.PEEK[]`.
- **Testing** (`testing.rs`, `cfg(test)` or feature `testing`): `FakeSmtp` (EHLO, AUTH PLAIN/LOGIN, optional required creds, records messages) and `FakeImap` (LOGIN, SELECT, UID SEARCH/FETCH/STORE, LOGOUT, UIDVALIDITY). The ignored `greenmail_round_trip` test runs against real GreenMail with `GREENMAIL=1`.
- Config structs redact passwords in `Debug`.
