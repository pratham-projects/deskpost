# Security

## Reporting

Email the maintainer privately (see the git log for the address). Do not open a public issue for
vulnerabilities. Expect an acknowledgement within a few days.

## Deployment checklist

- `JWT_SECRET` and `SD_SECRET_KEY`: distinct, from `openssl rand -base64 48`. The API refuses to start
  with short or placeholder values. Rotating `SD_SECRET_KEY` makes stored SMTP/IMAP passwords unreadable.
- Run with `SD_ENV=production` (default): secure cookies on, strict config validation.
- Terminate TLS at a reverse proxy (`deploy/Caddyfile`) and set `SD_TRUSTED_PROXIES` to the proxy address
  only, so client IPs in rate limits and logs cannot be spoofed.
- `FRONTEND_URL` lists exact origins for CORS; never `*`.
- Leave `SD_ALLOW_PRIVATE_MAIL_HOSTS=false` in production. It is the SSRF guard on user-supplied
  SMTP/IMAP hosts; enabling it lets project admins probe loopback and private networks.
- Set `SD_ALLOW_SIGNUP=false` after creating the first admin with `api bootstrap`, unless open signup is intended.
- Pass the bootstrap password through `SD_BOOTSTRAP_PASSWORD`, not `--password` (visible in `ps`).
- Set `SD_WEBHOOK_SECRET` if you expose the inbound mail webhook; requests are signature-checked.
- Postgres: dedicated role and password, not exposed publicly. Compose binds all ports to 127.0.0.1 and leaves `SD_ALLOW_PRIVATE_MAIL_HOSTS` unset (false).
- `.env` holds secrets: never commit it, `chmod 600`.
- Back up Postgres and `STORAGE_DIR` (attachments) together.

## Accepted risks

Known limits of this release, decided on rather than forgotten (R50/R51 review):

- **No orphan sweep for attachment files.** A crash between writing a file and inserting its row, or a failed delete after the row is gone, leaves an unreferenced file. Only `<STORAGE_DIR>/.tmp` is swept (files older than a day, by the housekeeping job). Compare `attachments.storage_key` with the store if disk use looks wrong.
- **Project delete vs. concurrent upload.** Storage keys are read before the project rows are deleted; an upload racing the delete can leave one file behind.
- **Declared types are only sniffed for programs.** Uploads are refused when the bytes look like a PE/ELF/Mach-O/script, but a file is not otherwise checked against its type (no full magic-byte match). Inbound mail and webhook attachments have no type allowlist; they are served download-only with `nosniff`.
- **Signed download links are bearer URLs.** `?token=` is valid for about 5 minutes, is not bound to a user and is not revoked when membership is removed. Access logs record the path only (the query string is not logged); keep any proxy in front from logging query strings too.
- **Sign-up answers 409 for an existing email** (account enumeration), limited by the per-IP sign-up rate limit.
- **DNS rebinding window on the connection test.** The test endpoints resolve and check the host, then connect; real sends and polls pin the checked address at connect time.
- **`/health/db` is unauthenticated.** It runs one trivial query; it is covered by the global request timeout and in-flight cap (120 s, 512 requests), not by a rate limit.
- **Development mode.** `SD_ENV=development` returns the refresh token in the JSON body and relaxes cookies. `SD_ENV` defaults to production; never set it in compose or systemd files for a real deployment.
- **Mail leases assume one clock.** The outbox lease (`batch x 45 s + 60 s`) keeps two API instances from sending one job twice; a worker that dies mid-send can still repeat that one mail after the lease. Jobs claimed five times without an outcome are given up.

## Built-in protections

Argon2 password hashing, encrypted mail credentials with per-field AAD, CR/LF header-injection rejection,
request size limits, tenant-scoped queries, `cargo deny` advisory/licence checks in `make ci`.
