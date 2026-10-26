# SendDesk (Rust) Plan

Multi-tenant support-ticket inbox. This repo is the **Rust backend + web UI**. The UI started as the
portfolio demo (mock API); the mock is replaced by a real Axum/Postgres API that serves the same
contract. Source of truth for behaviour: the closed-source product `Pratham-Jobs/sendesk`
(TypeScript/Bun/Elysia/Drizzle), cloned read-only at `~/code/sendesk-jobs` (the user's own code;
reading it is allowed and expected, but this repo is a rewrite, not a transliteration: fix what is
wrong, do not port bugs).

## 1. Goals

1. `apps/web` (Next.js, existing UI, `lib/store.ts` untouched where possible) talks to the Rust API for real.
2. No paid or proprietary services. Email out = any SMTP server; email in = IMAP polling **and** the
   signed webhook; attachments = local filesystem (S3 optional later, not now). Free options documented:
   Brevo/Resend/Gmail-app-password SMTP for production, Mailpit (dev inbox UI), GreenMail (SMTP+IMAP for tests).
3. Same quality bar as bunfiiliate: unit + property + integration (`#[sqlx::test]`) + e2e browser tests,
   security review pass, `make ci` green, local final gate before anything is pushed.

## 2. Workspace

```
crates/core     pure logic: ticket status machine, email address/subject/threading, reply stripping and
                body extraction (port of emailParser.ts / emailBodyExtractor.ts), secret box (AES-256-GCM),
                API-key hashing. No IO.
crates/db       sqlx + migrations + typed queries. Postgres 16 only (no PG18 SQL).
crates/storage  attachment store trait + filesystem driver (traversal-safe, size caps, signed download tokens)
crates/mail     SMTP sender (lettre), IMAP poller (async-imap), MIME parse (mail-parser), test SMTP sink
crates/api      Axum binary: routes, auth, middleware, config, workers (imap poll, auto-close, presence GC)
apps/web        Next.js UI (bun). NEXT_PUBLIC_DEMO=1 keeps the mock; default build talks to the API.
```

Rust edition 2024, Axum 0.8, SQLx 0.8, tokio. Money does not exist here; IDs are UUIDs (v7 minted in Rust).

## 3. API contract

`docs/API.md` (task R10) is derived from `apps/web/mock/router.ts` + `apps/web/lib/api/*.ts` (what the UI
needs) cross-checked with `~/code/sendesk-jobs/backend/src` (what the product does). Prefix `/api/v3`
(`API_VERSION` env, UI reads `NEXT_PUBLIC_API_VERSION`). Response envelope and cookie behaviour must match
what `apps/web/lib/api/client.ts` expects; any intentional deviation is recorded in API.md and the UI is
changed in the same task.

## 4. Hardening rules (learned on bunfiiliate, apply from the start)

- `JWT_SECRET` >= 32 bytes, reject placeholders. Argon2id with a concurrency semaphore. Constant-time compares.
- Access token 5 min + rotating refresh token in an HttpOnly cookie; reuse of a rotated token revokes the
  family (with a 10 s grace for parallel tabs); family capped at 90 days.
- Login / register / webhook / public widget endpoints rate limited (in-process sliding window) and body limited.
- Trusted-proxy aware client IP (`SD_TRUSTED_PROXIES`); never trust `X-Forwarded-For` from anyone else.
- Webhook secrets compared in constant time; email webhook requires a signed timestamp (5 min tolerance).
- Per-project SMTP/IMAP passwords encrypted at rest (AES-256-GCM, key from `SD_SECRET_KEY`), never returned by the API.
- Every query is tenant-scoped by project membership; a test per route proves another tenant gets 404/403.
- Attachments: size caps, filename sanitising, content-type allowlist for inline display, `Content-Disposition:
  attachment` otherwise, no path built from user input, quota per project.
- Outbound HTTP/SMTP/IMAP hosts configured by admins only; reject loopback/private targets unless `SD_ALLOW_PRIVATE_MAIL_HOSTS=1`.
- Unbounded anything is a bug: queues, request bodies, result pages, IMAP fetch batches.

## 5. Tasks

Git flow: `main` (prod) <- `staging` <- `feat/*`; `--no-ff` merges; never commit on main/staging except merge
commits and brain touch-ups by the coordinator. Short commit messages. **No push** until the user says so.

| id  | branch | scope | agent |
|-----|--------|-------|-------|
| R10 | feat/db-contract | `docs/API.md`; migrations 0001+ from the Drizzle schema (hardened); `sd-db` connect/migrate/queries; db tests | claude |
| R11 | feat/core | status machine, email address/subject/threading, reply stripping + body extraction (port the 705-line `emailParser.test.ts` cases), secret box, key hashing; proptest | claude |
| R12 | feat/storage | storage trait, fs driver, signed tokens, quota, traversal tests | claude |
| R13 | feat/mail | lettre sender (attachments, threading headers), IMAP poller + MIME to `InboundEmail`, hermetic SMTP sink for tests | claude |
| R20 | feat/api-auth | config, app skeleton, errors/envelope, auth (register/login/refresh/signout/me), roles, rate limit, trusted proxy | claude |
| R21 | feat/api-projects | projects, members, users/team, clients, templates, per-project email responses, SMTP/IMAP settings (encrypted) | claude |
| R22 | feat/api-tickets | tickets, messages, internal notes, status, assignment, search/filter/pagination, unread, viewers/presence, widget submit | claude |
| R23 | feat/api-mail-attach | attachments routes, outbound reply mail + auto-responses + email_logs, inbound webhook, IMAP poll worker, auto-close job | claude |
| R30 | feat/web-live | UI against real API: remove mock from default build, env wiring, fix contract drift, static export served by the API or Caddy | claude |
| R31 | feat/deploy-docs | Dockerfile, compose (postgres, app, mailpit), Caddyfile, systemd, README, SECURITY.md, `.env.example`, free-email guide | claude |
| R40 | feat/e2e | `scripts/e2e.sh`: postgres + GreenMail/Mailpit + api + web; browser flows with `e2e`; mail round-trip (customer email in -> ticket -> agent reply -> mail out) | claude |
| R50 | review | 3 read-only reviewers (auth/api, mail/storage, web/deploy) -> reports -> fix wave | claude |
| R60 | gate | `make ci` + `make e2e` + ocr delegate; merge staging -> main locally | coordinator |

Dependencies: R10, R11, R12, R13 parallel. R20 after R10+R11. R21, R22, R23 after R20 (R23 also after R12, R13).
R30 after R21+R22. R31 any time after R20. R40 after R23+R30. R50 after R40.

## 6. Local environment

Postgres 16 on port **5433** (`DATABASE_URL=postgres://pratham@localhost:5433/postgres`); per-worktree scratch
databases `s1..s4` for `#[sqlx::test]` so parallel workers do not collide. `.cargo/config.toml` pins
`RUST_TEST_THREADS=4`. Mailpit: `brew install mailpit` (done). GreenMail standalone jar from Maven Central into
`.tools/` (gitignored) for SMTP+IMAP in e2e; Java 26 is installed. No docker.
