---
title: Security
group: Operate
order: 9
description: What is hardened, the deployment checklist, and the accepted risks.
---

# Security

Report vulnerabilities privately by emailing the maintainer (the address is in the git log). Do not open a public issue.

## What is built in

- Argon2id password hashing behind a concurrency semaphore, with per-address and per-account sign-in throttles.
- Five minute access tokens plus a rotating, hashed refresh token. Reusing a rotated token revokes the family, except within a 10 second grace for parallel tabs. A family lasts at most 90 days.
- SMTP and IMAP passwords encrypted with AES-256-GCM with per-field associated data, key from `DP_SECRET_KEY`. Never returned by the API; `Debug` redacts them.
- Every query is scoped by project membership, with a test per route proving another tenant gets `404` or `403`.
- Rate limits on sign-in, sign-up, refresh, the public widget route and the webhook. Request bodies are capped.
- SMTP and IMAP hosts are admin-configured and loopback, private and link-local targets are rejected unless `DP_ALLOW_PRIVATE_MAIL_HOSTS` is true. Real sends and polls pin the checked address.
- Webhook signature is an HMAC over a timestamp and the body, constant-time compared.
- CR and LF header injection rejected. Attachments: filename sanitising, no user-built paths, program-looking files refused, `nosniff` and download-only for unsafe types.
- `cargo deny` advisory and licence checks run in `make ci`.

## Deployment checklist

- `JWT_SECRET` and `DP_SECRET_KEY`: distinct, from `openssl rand -base64 48`. The API refuses short or placeholder values.
- Keep `DP_ENV=production` (the default).
- Terminate TLS at a reverse proxy and set `DP_TRUSTED_PROXIES` to that proxy only.
- `FRONTEND_URL` lists exact origins, never `*`.
- Leave `DP_ALLOW_PRIVATE_MAIL_HOSTS=false` in production.
- Set `DP_ALLOW_SIGNUP=false` after `deskpost bootstrap`, unless open sign-up is intended.
- Pass the bootstrap password through `DP_BOOTSTRAP_PASSWORD`, not `--password`.
- Set `DP_WEBHOOK_SECRET` only if you expose the inbound webhook.
- Dedicated Postgres role, not exposed publicly. `.env` is `chmod 600` and never committed.

## Accepted risks

These risks were reviewed and accepted (from `SECURITY.md`):

- No orphan sweep for attachment files after a crash between writing a file and its row. Only `.tmp` is swept.
- A project delete racing an upload can leave one file behind.
- Uploads are sniffed only for programs. Inbound mail and webhook attachments have no type allowlist and are served download-only.
- Signed download links are bearer URLs valid for about 5 minutes, not bound to a user.
- Sign-up answers `409` for an existing email, limited by the per-IP sign-up rate limit.
- A DNS rebinding window exists on the connection-test endpoints.
- `/health/db` is unauthenticated, covered by the global timeout and in-flight cap.
- `DP_ENV=development` returns the refresh token in the body. Never use it in production.
- Mail leases assume one clock; a worker that dies mid-send can repeat that one mail.

## Rotating secrets

| Secret | Effect |
| --- | --- |
| `JWT_SECRET` | Outstanding access tokens and signed download links stop working. Sessions survive. |
| `DP_SECRET_KEY` | No re-seal tool: every project's mail passwords must be re-entered. Rotate only on suspected compromise. |
| `DP_WEBHOOK_SECRET` | Update the sender at the same time or inbound webhooks get `401`. |
