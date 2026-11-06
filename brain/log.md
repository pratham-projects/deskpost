# Changelog

## [2026-11-07] R50 backend fixes

- **Creation**: SSRF check at connect time, IMAP timeouts and size-bounded fetch, ingest sanitising and sender check, signup off by default, client scope on tickets, status race, no reflected text in auto-responses, mailbox approval by a global admin (migration 0003), attachment mail after commit (same rule as `/messages`), streamed downloads, summed attachment size cap, per-account login throttle, bounded hashing queue.
- **Decision**: attachment upload with `sendEmail` no longer fails or rolls back when mail fails; it saves, then logs the failure like replies. A mailbox is approved for webhook routing by the platform operator (global admin); a tenant cannot prove an address, because a login or a mailed code both pass through a server the tenant picks.
- **Lesson**: [login to a chosen server proves nothing](/wisdom/time-traps.md).

## [2026-11-06] R50 fix web-deploy

- **Fixed**: H1 web image is now `caddy:2-alpine` serving the static export (`deploy/web.Caddyfile`, proxies `/api` to `app:3000`, same origin); host `deploy/Caddyfile` serves `/srv/senddesk/web` directly. H2 one name `NEXT_PUBLIC_API_URL` (empty = same origin) in Dockerfile, compose, `.env.example`. H3 no localStorage tokens; access token in memory, session restored via `/auth/refresh` cookie (demo-mock restore gated on `NEXT_PUBLIC_USE_MOCK`). M1 `lib/debug.ts` no-op in prod replaces all `console.log`. M2 typed `AuthExpiredError`, match 401 + TOKEN_* codes only, `ApiRequestError` carries `details`. M3 single-flight + `navigator.locks` refresh. M4 CSP Report-Only + Permissions-Policy. M5 widget `Referrer-Policy: no-referrer`. M6 UI types = server list, contract test `e2e/tests/attachment-types.spec.ts`. M7 blob URL revoke + stale guard. L1 jar sha256 pin, L2 e2e refuses non-local db (`E2E_DATABASE_URL`), L3 trap INT/TERM + child kill, L5 compose env allow-list, mailpit profile `dev`, `.dockerignore`, L6 proxy health/timeouts.
- **Not done / notes**: M5 per-project frame-ancestors needs API change (crates untouched). M6: server serves PDF download-only (`content_type.rs`), so viewer "unknown" for PDF is correct; review claim wrong. Systemd hardening extras, image digest pins, API `healthcheck` subcommand (no curl) and 40MB body split in Caddy left. 422 field `details` now reach `ApiRequestError.details`, forms do not render them yet. Docker not installed: image and compose not built; Caddyfiles validated with `caddy validate`.
- **Lesson**: `next start` + `output: export` fails at boot and e2e never touches the Docker target; add a docker smoke when docker is available.
- **Verify**: `make web` ok, `make e2e` 6/6 pass.

## [2026-11-06] R40 e2e

- **Creation**: `scripts/e2e.sh` (scratch db `sd_e2e` on :5433, GreenMail 2.1.15 jar in `.tools/`, api binary with migrate+bootstrap, Caddy serving `apps/web/out` and proxying `/api` + `/health`, then Playwright) and `e2e/` (`@playwright/test` on system Chrome, imapflow, nodemailer). Flows: bad/good login, create project in UI, widget ticket, agent reply -> mail out, customer email -> IMAP poll -> ticket -> reply -> mail out.
- **Decision**: ports api 3100, web 4100, SMTP 3025, IMAP 3143 (script refuses busy ports). Mail project is created via API because the UI create-project dialog has no IMAP fields (gap, not a bug in scope).
- **Lesson**: reply mail subject is `Re: <project> Support - [Ticket #N]`, not the customer subject; match on the ticket number. No api/web bug surfaced by these flows.

## [2026-11-06] R31 deploy docs

- **Creation**: multi-stage `Dockerfile` (targets `api`, `web`), `docker-compose.yml` (postgres, migrate, app, web, mailpit), `deploy/Caddyfile`, `deploy/senddesk-api.service`, `.env.example` (every var in `config.rs` and `mail_config.rs`), `SECURITY.md`, new `README.md` linking the free-email guide.
- **Decision**: web ships as `next start` under bun, not a static export, because `apps/web/next.config.mjs` is out of scope. `NEXT_PUBLIC_*` are build args.
- **Lesson**: docker is not installed locally, so images and compose were not built; only YAML syntax and env-var coverage were checked.

## [2026-11-06] R30 web live

- **Creation**: `apps/web/lib/api/config.ts` (`NEXT_PUBLIC_API_URL`, default same origin; `NEXT_PUBLIC_API_VERSION`, default v3); `output: "export"` to `apps/web/out`; `.env.example`.
- **Decision**: mock API and demo badge only with `NEXT_PUBLIC_USE_MOCK=1` (turbopack alias swaps in a stub otherwise; verified absent from the default bundle). Auth client reads the `{success,data,error}` envelope; signup then signs in (API returns no tokens); widget takes `/widget?project=&key=` and sends `X-Project-Key`; attachment upload reads `.data`; role picker removed (first user is admin). Unused `ui/chart`, `resizable`, `calendar` deleted, `ignoreBuildErrors` removed, so `make web` typechecks clean. `vercel.json` and Vercel analytics dropped.
- **Lesson**: [unload-time DELETE](/wisdom/time-traps.md) and bundler dead-code.

## [2026-11-06] R23 api mail attach

- **Creation**: attachments routes, inbound webhook and IMAP poll worker, outbound reply mail, auto-response outbox (migration 0002), `email_logs`; see [API tickets](/architecture/api-tickets.md).
- **Decision**: merged with R22 by keeping R23's `Mailer` (SMTP + recording double) in `AppState`; ticket replies still save first and mail after commit, then store the sent headers on the message (`messages::set_email_headers`). Migration renamed 0023 to 0002. `deny.toml` allows CDLA-Permissive-2.0 and ignores RUSTSEC-2026-0441 (pin-utils via async-imap).
- **Lesson**: two workers building the same seam in parallel need the trait named in the plan; reconcile at merge, not at review.

## [2026-10-27] R22 api tickets

- **Creation**: tickets (create admin + widget, list/search/counts, get, patch, delete), messages and internal notes, unread, viewers with GC task, `OutboundMailer` seam; see [API tickets](/architecture/api-tickets.md). 50 new integration tests.
- **Decision**: status changes go through the sd-core machine (closed only reopens to pending, same status is a no-op), so API.md 7.3 "any to any" was corrected. Mail is sent after commit and failures are recorded, replacing "mail first, 502" of API.md 7.4 (R23 may revisit). Delete stays a hard delete (API.md deviation 16), not a soft delete.
- **Lesson**: [axum multipart and query rejections](/wisdom/time-traps.md).
## [2026-10-27] R21 api projects

- **Creation**: routes for projects, members, users, clients, templates, email responses and SMTP/IMAP settings with probes ([architecture](/architecture/projects-api.md)); `mail_guard` SSRF policy; `sd_mail::check_smtp`; db `update_with_mail`, `admin_update_guarded`, `delete_guarded`, `email_responses::project_of`.
- **Decision**: TLS derived from port (no schema change); passwords sealed with project+field AAD; `POST /users/me/password` and read-only `/clients` added to `docs/API.md`.
- **Lesson**: [two-step writes need one transaction](/wisdom/time-traps.md).

## [2026-10-27] R20 api auth

- **Creation**: `crates/api` (lib + `api` binary: serve, migrate, bootstrap), config, envelope/errors, HS256 JWT, Argon2id hasher, sliding-window limiter, trusted-proxy client IP, auth routes, extractors, health; see [API crate](/architecture/api.md). 100+ tests, including a reusable tenant fixture.
- **Decision**: `/auth/config` kept (reports `signupEnabled`), `/health` split into liveness and `/health/db`, default body cap 64 KiB, new `METHOD_NOT_ALLOWED` code; all recorded in `docs/API.md`.
- **Lesson**: [refresh grace must not clear the cookie](/wisdom/time-traps.md).

## [2026-10-27] R12 storage

- **Creation**: `sd-storage` (Storage trait, FsStorage, DownloadSigner, check_quota), [architecture page](/architecture/storage.md) with TS weaknesses. S3 driver deferred.
## [2026-10-27] R11 sd-core

- **Creation**: `crates/core` modules status, email_addr, threading, body, secret, api_key, filename, content_type, `constant_time_eq`; see [sd-core](/architecture/core.md). All `emailParser.test.ts` cases (and the extractor cases 1-20) rewritten as Rust tests plus proptest properties; 148 tests.
- **Decision**: TS bugs fixed instead of ported (RFC 2047 spoof, `From:` over-cut, bounce prefix match, key derivation); PDF kept download-only.
## [2026-10-27] R13 mail crate

- **Creation**: `sd-mail` (lettre sender, mail-parser inbound, async-imap `Mailbox`/`poll`, `testing` fakes), `docs/free-email.md`, [architecture/mail.md](/architecture/mail.md).
- **Lesson**: [mail-parser and IMAP gotchas](/wisdom/time-traps.md).
## [2026-10-27] R10 db + contract

- **Creation**: `docs/API.md`, `crates/db` (sd-db) with migration 0001, 13 query modules, 69 DB tests. See [DB crate and API contract](/architecture/db-contract.md).
- **Decision**: templates and clients are per project; API key hashed; `/clients` CRUD dropped; envelope `{success,...}` means R30 must change UI auth `status` checks.

## [2026-10-27] R00 scaffold

- **Creation**: Rust workspace (sd-core, sd-db, sd-mail, sd-storage, api), Makefile, CI, deny.toml, `docs/PLAN.md`, brain. Next.js demo moved to `apps/web`.
