---
title: Configuration
group: Configure
order: 4
description: Every environment variable the API reads, with defaults.
---

# Configuration

The API is configured through environment variables only. `.env.example` lists every variable read by `crates/api/src/config.rs` and `mail_config.rs`. There is no dotenv loader: export the variables (`set -a; . ./.env; set +a`), use compose, or use a systemd `EnvironmentFile`.

## Required

| Variable | Notes |
| --- | --- |
| `DATABASE_URL` | PostgreSQL 16. Compose derives it from `POSTGRES_PASSWORD`. |
| `JWT_SECRET` | Signs access tokens. 32 bytes or more, placeholders rejected. |
| `DP_SECRET_KEY` | Seals stored SMTP and IMAP passwords. Must differ from `JWT_SECRET`. |

## Core

| Variable | Default | Notes |
| --- | --- | --- |
| `DP_ENV` | `production` | `development` relaxes cookies and returns the refresh token in the body. |
| `PORT` | `3000` | The API binds `0.0.0.0:$PORT`. |
| `API_VERSION` | `v3` | Routes live under `/api/<API_VERSION>`. Keep `NEXT_PUBLIC_API_VERSION` equal. |
| `FRONTEND_URL` | | Comma-separated CORS origins. Exact origins, never `*`. |
| `COOKIE_SECURE` | true in production | Secure cookies need HTTPS. |
| `DP_TRUSTED_PROXIES` | empty | IPs or CIDRs of your reverse proxy. Only these may set `X-Forwarded-For`. |
| `STORAGE_DIR` | `./data/storage` | Attachment root. Production: `/var/lib/deskpost/storage`. |
| `DP_ALLOW_SIGNUP` | `false` | `true` opens public `POST /auth/signup`. |
| `DP_FIRST_USER_ADMIN` | `false` | With sign-up open, the first account becomes admin. Prefer `deskpost bootstrap`. |
| `DP_ALLOW_PRIVATE_MAIL_HOSTS` | `false` | Local mail servers only (Mailpit, GreenMail). |
| `RUST_LOG` | `info` | `api=debug,sqlx=warn` for troubleshooting. |

## Limits

| Variable | Default |
| --- | --- |
| `DP_BODY_LIMIT_BYTES` | `65536` |
| `DP_UPLOAD_LIMIT_BYTES` | `27262976` |
| `DP_WEBHOOK_LIMIT_BYTES` | `37748736` |
| `DP_MAX_FILE_BYTES` | `10485760` |
| `DP_MAX_REQUEST_BYTES` | `26214400` |
| `DP_MAX_FILES` | `20` |

## Mail

| Variable | Default | Notes |
| --- | --- | --- |
| `DP_WEBHOOK_SECRET` | unset | Enables the signed inbound webhook. Unset means disabled. |
| `DP_MAIL_PLAINTEXT` | `false` | Never use TLS to project mail servers. Local dev and tests only. |
| `DP_IMAP_POLL_SECS` | `60` | Poll interval. |
| `DP_IMAP_CONCURRENCY` | `4` | Mailboxes polled in parallel. |
| `DP_BOOTSTRAP_PASSWORD` | | Read by `deskpost bootstrap` only. Remove it afterwards. |

## Compose and web

`POSTGRES_PASSWORD` seeds the database role. `PUBLIC_URL` is baked into the web image as `NEXT_PUBLIC_API_URL`; leave it empty so the UI and API share one origin. `DP_RATE_LIMIT_SCALE` multiplies per-address rate limits and is only for load tests.

> `NEXT_PUBLIC_*` values are baked in at image build time, not read at runtime.

## Commands

```bash
deskpost serve       # HTTP server and workers
deskpost migrate     # forward-only migrations
deskpost bootstrap --email you@example.com --name "You"   # first admin, password from DP_BOOTSTRAP_PASSWORD
```
