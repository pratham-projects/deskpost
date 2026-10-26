# Free email with `sd-mail`

No AWS. Outbound is plain SMTP, inbound is IMAP polling (or the signed webhook). Every call takes its
own settings, so each project can use a different provider. Passwords are stored encrypted by the API
and never logged (`Debug` on the config structs redacts them).

TLS modes (`TlsMode`): `StartTls` (connect plain, upgrade; usual port 587 / IMAP 143), `Implicit`
(TLS from the first byte; 465 / 993), `None` (local test servers only).

## Outbound SMTP

| Provider | host | port | tls | username | password | from address |
|----------|------|------|-----|----------|----------|--------------|
| Gmail (app password) | `smtp.gmail.com` | 587 | StartTls (465 Implicit also works) | full Gmail address | 16-char [app password](https://myaccount.google.com/apppasswords) (needs 2-step verification) | the same Gmail address (Gmail rewrites others); ~500 mails/day |
| Brevo (free: 300/day) | `smtp-relay.brevo.com` | 587 | StartTls | SMTP login shown in Brevo > SMTP & API | SMTP key (not the account password) | a sender you verified in Brevo |
| Resend (free tier) | `smtp.resend.com` | 465 (or 587) | Implicit (587: StartTls) | literally `resend` | an API key | an address on a domain you verified in Resend |
| Mailgun (free tier) | `smtp.mailgun.org` | 587 | StartTls | `postmaster@your-domain` | the domain's SMTP password | an address on the verified domain (sandbox domains only reach authorised recipients) |
| Mailpit (dev inbox) | `127.0.0.1` | 1025 | None | none | none | anything |
| GreenMail (tests) | `127.0.0.1` | 3025 | None | none | none | anything |

```rust
let mut cfg = SmtpConfig::new("smtp.gmail.com", 587, TlsMode::StartTls, "desk@gmail.com");
cfg.username = Some("desk@gmail.com".into());
cfg.password = Some(app_password);
cfg.from_name = Some("Acme Support".into());
let message_id = sd_mail::send(&cfg, &OutboundEmail {
    to: vec!["customer@example.org".into()],
    subject: "Re: Printer".into(),
    text: Some("...".into()),
    html: Some("<p>...</p>".into()),
    in_reply_to: Some(parent_message_id),   // keep the customer's thread
    references: thread_message_ids,
    ..Default::default()
}).await?;
```

Store the returned Message-ID on the ticket message: the customer's reply carries it in
`In-Reply-To`/`References`, which is how replies find their ticket.

Any CR/LF or control character in a subject, address, name, id or custom header is rejected
(`Error::HeaderInjection`); custom headers must be ASCII and cannot override From/To/Subject/etc.
`io_timeout` bounds each SMTP command, `total_timeout` the whole send.

## Inbound IMAP

| Provider | host | port | tls | username | password | notes |
|----------|------|------|-----|----------|----------|-------|
| Gmail | `imap.gmail.com` | 993 | Implicit | full address | app password | IMAP is on by default |
| Any mailbox host | per provider | 993 / 143 | Implicit / StartTls | | | forwarding Brevo/Resend/Mailgun inbound is webhook-based instead (use the signed webhook) |
| GreenMail (tests) | `127.0.0.1` | 3143 | None | user part of the account | its password | |
| Mailpit | no IMAP | | | | | Mailpit is SMTP-in plus a web UI only; use GreenMail for the poller |

```rust
let mut mb = ImapMailbox::connect(&ImapConfig::new("imap.gmail.com", 993, TlsMode::Implicit, user, pass)).await?;
let out = sd_mail::poll(&mut mb, saved_state, &PollOptions { batch_size: 50, mark_seen: true, ..Default::default() }).await?;
// persist out.state (uid_validity + last_uid); handle out.emails; if out.has_more poll again now
// else sleep for the poll interval and reconnect.
```

- Each `poll` fetches at most `batch_size` messages with `BODY.PEEK[]`, so nothing is marked read unless `mark_seen`.
- A message over the caps (`Limits`: message, per attachment, attachment count, header count) or
  unparseable comes back as `Err(ParseError)` for its UID and is still consumed, so it cannot wedge the poller.
- If the server's UIDVALIDITY changes, `last_uid` restarts from 0 and `validity_reset` is set: dedupe by `message_id`.
- Messages without a Message-ID get a stable hash id, so a re-fetch dedupes.

## Local testing

- **Mailpit** (`brew install mailpit`; `mailpit`): SMTP on 1025, web UI on http://localhost:8025. Use it to look at what the app sends.
- **GreenMail**: SMTP 3025 + IMAP 3143 with both sides sharing mailboxes (see the header of
  `greenmail_round_trip` in `crates/mail/src/tests/imap.rs`):
  `java -Dgreenmail.setup.test.smtp -Dgreenmail.setup.test.imap -Dgreenmail.hostname=127.0.0.1 -Dgreenmail.users=desk:pw@example.com -jar .tools/greenmail.jar`
  then `GREENMAIL=1 cargo test -p sd-mail -- --ignored`.
- **In-process fakes** (no setup): `sd_mail::testing::{FakeSmtp, FakeImap}`, behind `cfg(test)` or
  `--features testing` for other crates (`sd-mail = { workspace = true, features = ["testing"] }` in dev-dependencies).
