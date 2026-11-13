---
title: Email
group: Features
order: 5
description: SMTP out, IMAP or signed webhook in, threading, and free provider settings.
---

# Email in and out

Outbound is plain SMTP and inbound is IMAP polling or a signed webhook, with no AWS account involved. Each project has its own settings, so projects can use different providers. Passwords are encrypted at rest with AES-256-GCM and never returned by the API.

TLS follows the port: `465` and `993` mean implicit TLS, anything else is STARTTLS.

## Outbound

Replying queues a job in the same transaction as the message. A worker sends it over the project's SMTP server every 15 seconds, adding `In-Reply-To` and `References` so the customer's reply threads back. The request never waits on SMTP.

CR, LF and control characters in any header value are rejected. Replies carry a subject like `Acme Support - [Ticket #42]`.

Per-project auto-replies exist for two events, `created` and `resolved`, one entry each, switchable.

## Inbound by IMAP

Configure the mailbox on the project. The API polls every `SD_IMAP_POLL_SECS` (60), fetching at most 50 messages per round with `BODY.PEEK`, so the poller does not mark mail as read. A message over the size caps or unparseable is consumed, so it cannot wedge the poller. If the server's `UIDVALIDITY` changes the cursor restarts and duplicates are dropped by `Message-ID`. IMAP needs no approval: the credentials prove ownership.

## Inbound by webhook

`POST /api/v3/incoming-email` with two headers:

```
X-Webhook-Timestamp: <unix seconds, within 5 minutes>
X-Webhook-Signature: <hex HMAC-SHA256 of "{timestamp}.{raw body}" with SD_WEBHOOK_SECRET>
```

The mail is routed to the project whose `smtpEmail` equals the recipient. Because a tenant cannot prove ownership of an address, a global admin approves it once with `POST /api/v3/projects/{id}/mailbox/approve`. Changing the project's address or mail servers clears the approval.

## Threading and reply stripping

A message joins an existing ticket through the `[Ticket #N]` tag in the subject or the `In-Reply-To` and `References` headers, only when the sender is the ticket's client or an address already on the ticket. Otherwise it opens a new ticket. Quoted blocks, "On … wrote:" lines in several languages, forwarded headers and signatures are stripped, and HTML-only replies are handled. A reply reopens a resolved or closed ticket. Auto-responders (`mailer-daemon`, `postmaster`, `Auto-Submitted`) are ignored.

## Free providers

| Provider | Host | Port | TLS | Username | Password |
| --- | --- | --- | --- | --- | --- |
| Gmail | `smtp.gmail.com` | 587 | STARTTLS | full address | 16-char app password (needs 2-step verification); about 500 mails a day |
| Brevo | `smtp-relay.brevo.com` | 587 | STARTTLS | SMTP login | SMTP key; 300 a day free |
| Resend | `smtp.resend.com` | 465 | implicit | `resend` | an API key |
| Mailgun | `smtp.mailgun.org` | 587 | STARTTLS | `postmaster@your-domain` | the domain's SMTP password |
| Mailpit (dev) | `127.0.0.1` | 1025 | none | none | none |

The from address must be one the provider accepts: the same Gmail address, a sender verified in Brevo, an address on a domain verified in Resend or Mailgun. For IMAP, Gmail is `imap.gmail.com` on 993 with the same app password. Brevo, Resend and Mailgun inbound is webhook-based.

## Local testing

Run Mailpit (`mailpit`, or compose `--profile dev`) and open http://localhost:8025 to see everything the app sends. Mailpit has no IMAP; the repo's tests use GreenMail and in-process fakes for the poller. Local mail servers need `SD_ALLOW_PRIVATE_MAIL_HOSTS=true`.
