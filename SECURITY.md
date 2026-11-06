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
- Postgres: dedicated role and password, not exposed publicly. Compose binds all ports to 127.0.0.1.
- `.env` holds secrets: never commit it, `chmod 600`.
- Back up Postgres and `STORAGE_DIR` (attachments) together.

## Built-in protections

Argon2 password hashing, encrypted mail credentials with per-field AAD, CR/LF header-injection rejection,
request size limits, tenant-scoped queries, `cargo deny` advisory/licence checks in `make ci`.
