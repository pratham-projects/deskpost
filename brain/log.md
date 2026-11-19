# Changelog

## [2026-11-19] First push, CI shakedown, rc.2 (fix/ci-runner, fix/deploy-check-stat, fix/release-version)

- **Done**: GitHub repo renamed `sendesk` -> `deskpost`, `main` + `staging` pushed, Pages on (site live at `/deskpost/`). First real GitHub CI runs found three bugs local runs could not: (1) `ubuntu-latest` is 26.04 (pg_dump 18 vs server 16, Playwright unsupported) -> runners pinned to `ubuntu-24.04` (not `release.yml`, dist-generated, uses 22.04); (2) `deploy-check.sh` file-mode test used `stat -f %Lp` first, which on GNU means filesystem stat and "succeeds" with junk -> GNU `stat -c %a` first; (3) tag `v0.1.0-rc.1` failed `dist host`: tag must equal the crate version, so version is `0.1.0-rc.2`. rc.1 tag deleted.
- **Lesson**: a pushed tag is a one-way door: before tagging, run `dist host --steps=create --tag=<tag>` locally. Never use `x || y` stat fallbacks where the first form can succeed wrongly.

## [2026-11-19] Prod push prep (fix/lettre-advisory)

- **Done**: gate rerun on main tree found `cargo deny` failing on new RustSec notices. `lettre` 0.11.23 -> 0.11.24 (Cargo.lock) cleared the four lettre/email advisories. `chrono` unmaintained notice (RUSTSEC-2026-0543, no upgrade, hard dep of sqlx + async-imap) ignored in `deny.toml` with reason.
- **Gate**: `make ci`, `make e2e` (20), `make deploy-check` green. Local pg on 5432 (pg16 brew); the old 5433 instance is gone, use `DATABASE_URL=postgres://pratham@localhost:5432/postgres`.
- **Lesson**: advisories land on a calendar, not a commit. Rerun `cargo deny check` right before any tag, not just in CI history.

## [2026-11-16] staging merged into main (local)

- **Done**: `main` = Deskpost pre-release (`--no-ff`, tree identical to staging). Not pushed, not tagged: origin/main is still the old demo commit.
- **Next (manual)**: rename GitHub repo to `deskpost`, push `main` + `staging`, enable Pages (Actions) and public ghcr, then tag `v0.1.0-rc.1`.

## [2026-11-16] New-ticket notice no longer reflects raw sender text (fix/new-ticket-notice-excerpt)

- **Done**: security review flagged the `new_ticket` mail to members: it sent from the project's trusted mailbox with the customer's subject and address verbatim (phishing text from a trusted sender). Now both are quoted under an "unverified, do not follow links" line, cut to 120 chars, control chars stripped, `://` and `www.` defanged (`mail_out::untrusted_excerpt`, unit tests). Auto-responses to customers already reflected nothing.
- **Gate**: `make ci` green.

## [2026-11-16] Pre-release pass (feat/pre-release)

- **Done**: binary `api` -> `deskpost` (crate, bin, Dockerfile, unit ExecStart, installer `deskpost-installer.sh`, scripts, docs, site; lib name stays `api`). Customer reply reopens resolved/closed to `in_progress` (core `on_customer_reply` now matches db; manual reopen of closed stays `pending`); auto-close default 7 days verified in migration, docs, site. `LICENSE` (AGPL-3.0), `CHANGELOG.md`, `docs/releasing.md` (main, first tag `v0.1.0-rc.1`), CI jobs `docker-build` (api+web, no push) and `deploy-check`; `dist plan` + local `dist build` ok. Compose gets `name: deskpost`.
- **Docker**: colima installed (`brew install colima docker docker-compose docker-buildx`, plugin dir in `~/.docker/config.json`). Images build; stack ran end to end (migrate 7, bootstrap, sign-in, projects) then torn down. Site security/FAQ/install copy synced with new accepted risks (widget frameable, no per-sender intake throttle) and the docker/CI facts. History secret scan clean.
- **Found + fixed**: `.env.example` inline `# comments` became the value in Compose (`DP_TRUSTED_PROXIES` failed `migrate`); comments now on their own lines. Hero used undefined `gh` after the LICENSE-link edit and crashed the site (smoke caught it); links now point at `blob/main`. docs/API.md drift (PDF is download-only, key layout `ab/cd/...`).
- **Gate**: fmt, `make ci`, `make e2e` (20), `make deploy-check`, site build + smoke at `/deskpost/` green.
- **Tribe**: codex login revoked (`codex login` needed); codex tasks rerouted to claude1 by editing `.coord/board.json`. Workers must run the site smoke when they touch `site/`.
- **Left**: smoke.sh against the host Caddy front (headers live in `deploy/Caddyfile`, not the web image); first real GitHub run of the new CI jobs; manual items in the release checklist.

## [2026-11-16] Review findings audit H/M/L (feat/pre-release)

- **Done**: every finding in `reviews/{auth-api,mail-storage,web-deploy}.md` re-checked against the tree; `reviews/STATUS.md` has id, status and file:line evidence. Nearly all were already fixed (R50 fix branches, R51 leftovers) or accepted in `SECURITY.md`.
- **Fixed now**: `shouldNotify` sent no mail although `docs/API.md` promises a new-ticket mail to flagged members. New outbox kind `new_ticket` (migration `0007`), `mail_out::enqueue_team_notice` on widget, admin and inbound ticket creation, one job per active flagged member, subject and sender only (no description), bounded by the 1000-pending cap. systemd unit: `ProtectKernelModules`, `RestrictNamespaces`, `CapabilityBoundingSet=`, `UMask=0077`.
- **Left open (see STATUS.md)**: `imap_last_error` has no API reader; no per-sender ticket-creation throttle on the mail paths; docs drift in `docs/API.md` (PDF inline, on-disk key, endpoint table) and a widget note in `SECURITY.md`; curl healthcheck, image digest pins, Caddy body-cap split.
- **Lesson**: [a new outbox kind changes every test that counts `mail_outbox`](/wisdom/time-traps.md); count by `kind`.

## [2026-11-16] Rebrand SendDesk -> Deskpost (feat/rebrand)

- **Done**: full rename to the site's name. Display name, `senddesk`/`sendesk` -> `deskpost` (systemd unit `deskpost-api`, `/etc|/var/lib|/srv/deskpost`, db/user `deskpost`, ghcr `deskpost-api`/`deskpost-web`, backup names, mock/cookie keys, Pages base `/deskpost/`), env `SD_*` -> `DP_*`, crates `sd-*` -> `dp-*`, scratch dbs `dp_*`. Site "site-only rename" disclaimers removed (intro, FAQ, brand.js, PRODUCT.md, DESIGN.md).
- **Kept on purpose**: legacy upstream refs (`~/code/sendesk-jobs`, `Pratham-Jobs/sendesk`); HMAC labels `sd-dl-v1`, `sd-download-signer-v1` (internal, changing them gains nothing); binary name `api`.
- **Found + fixed**: `make deploy-check` was already failing on staging: compose and `.env.example` lacked `DP_SYSTEM_SMTP_*` (password reset could not be enabled in Docker) and compose lacked `DP_RATE_LIMIT_SCALE`. Added.
- **Gate**: `make ci`, `make e2e` (20), `make deploy-check`, site build + smoke green.
- **Lesson**: rename shifts rustfmt import order (`sd_db` vs `dp_db`): run `cargo fmt --all` before `make ci`. `deploy-check` backup test needs `pg_dump` matching the server (`PATH=/opt/homebrew/opt/postgresql@16/bin:$PATH`). Site smoke needs `CHROME_PATH` to the cached Playwright chromium.
- **Manual**: rename GitHub repo `pratham-projects/sendesk` -> `deskpost` before first release (URLs in code already say `deskpost`).

## [2026-11-13] Site copy humanizer pass

- **Done**: every string on the site (`brand.js`, `content/docs/*.md`, kit headings, FAQ, footers) read against blader/humanizer v4. Removed staged headings ("Everything it does, listed.", "Built in the open."), "not X, Y" footers, fragment rows, decorative bold labels, inflated blurbs ("Hardened from the first commit."), stock words. No facts, numbers, commands or claims changed. Smoke test now asserts the hero screenshot instead of the ASCII canvas.
- **Rule kept**: contrasts stay where both halves carry information (`404`, not `403`). Docs stay plain reference text.

## [2026-11-13] Hero product screenshots

- **Done**: Hero: ASCII scene replaced by a framed product screenshot (ticket inbox, mock build via `NEXT_PUBLIC_USE_MOCK=1 next dev`), light/dark `<picture>` by OS theme, "sample data" caption. `brand.shot` opts a site in; no shot keeps the ASCII canvas.
- **Lesson**: capture at 1280x800 (dsf 2) then `cwebp -q 84 -resize 2000 0`; at 1120 Deskpost's responsive layout squeezes the ticket pane. Never shoot real data (`~/.learnpuppy` holds the user's own).

## [2026-11-13] Site theme pass

- **Done**: Site restyled to product UI: cool paper + navy primary from `apps/web/app/globals.css`, Geist Sans headings (GeistVariable.woff2 added, OFL), 10px radius, light-first, dark follows OS. Pixel type only on wordmark + ASCII scene. Branch `feat/site-ui-theme`, merged to staging.

## [2026-11-07] R2 release automation (feat/release)

- **Done**: cargo-dist workflow (binary `api`, 4 targets, shell installer, sha256), ghcr image workflow, compose and env release assets, `docs/releasing.md`.
- **Lesson**: `dist plan` passes without `[profile.dist]`, but `dist build` fails with "profile `dist` is not defined". Always run one local `dist build --target aarch64-apple-darwin`, not just plan. Fixed in `Cargo.toml`; local build now produces the tarball and sha256.
## [2026-11-07] S2 marketing + docs site "Deskpost" (feat/site)
- **Review pass**: finish reviewer round 2 = ship. Denser inbox scene, install headline `Run it your way.`, pre-release callout in install doc, Apple M4 on the p50 row, DESIGN.md added.

- **Done**: `site/` Vite vanilla marketing + docs site, brand name Deskpost (site-only rename; repo and binary stay `deskpost` / `api`). Amber skin on the shared kit copied from bunfiiliate (`scripts/sync-kit.sh`), `inbox` ASCII scene, 10 docs pages, Pages workflow, smoke test.
- **Decision**: install tabs lead with Docker Compose from a clone and Source; Images and Binary tabs say "not published yet" until a release exists (they flip automatically once the GitHub API lists one).
- **Open**: README and docs still say Deskpost. Auto-close default (7 days) and customer-reply reopen status (`pending` in core vs `in_progress` in SQL) disagree in the code; the site only says "reopens".

## [2026-11-07] Merge feat/webload, worktree cleanup

- **Merged**: `feat/webload` into `staging` (--no-ff). Conflicts: both branches added migration `0004`; webload's index moved to `0006_tickets_updated_idx.sql`, `migrate_twice_is_a_no_op` expects 6, brain tables/log kept both sides. Worktree and branch removed; one checkout left.
- **Gate**: `make ci` green after the merge (first run failed on a stale embedded migration list; db crate rebuilt).
- **Lesson**: [parallel branches collide on migration numbers, and `sqlx::migrate!` does not rebuild when a file is only added](/wisdom/time-traps.md).
- **Housekeeping**: dropped unreferenced scratch databases (15 GB); `main` merge still awaits approval.

## [2026-11-07] P4 web e2e + load

- **Creation**: Playwright specs for every screen (`e2e/tests/screens.spec.ts`; fixes for templates project scope, attachment names, api key display, stale login copy). `scripts/load.sh` (oha on a release API, scratch db `dp_load`, seeds 500 tickets) and `docs/PERFORMANCE.md`. `DP_RATE_LIMIT_SCALE` (default 1) multiplies per-address quotas for load runs. Migration `0006` (renumbered at merge; 0004/0005 are outbox reply and password resets) index `(project_id, updated_at DESC, id DESC)` for the inbox list.
- **Decision**: load runs lift rate limits by env instead of spoofing many source addresses; production keeps scale 1.
- **Lesson**: oha counts requests in flight at the deadline as errors; pass `-w`. Replies with `sendEmail` default and no SMTP log a warning per request, so load scenarios send `sendEmail:false`.

## [2026-11-07] P1 password reset

- **Creation**: single-use hashed reset tokens (migration 0004, `crates/db/src/resets.rs`), forgot/reset routes with own rate rules plus per-address mail cap, reset revokes all sessions, system SMTP mail, token carried in URL fragment and scrubbed, pages `/forgot-password` and `/reset-password`, spec `e2e/tests/password-reset.spec.ts`.
- **Lesson**: e2e specs share one loopback IP, so the per-IP login limit (10/min) is shared; sign-in helpers in later specs wait out 429. A hash-only `goto` to the same path does not reload, so replay tests must leave the page first.
- **Verify**: `make ci` ok, `make e2e` 8/8 pass.

## [2026-11-07] R51 review leftovers (P2)

- **Fixed (M8)**: agent replies go through the persisted outbox. `post_agent` queues message + `pending` log + `reply` job in one transaction (migration 0004: `mail_outbox.message_id`, `sent_by_user_id`, kind `reply`); `run_outbox_once` sends via `state.mailer` with retry/backoff; `/messages` and attachment upload share the path, both answer 201, no SMTP in the request. `docs/API.md` section 6.7 and 9 updated.
- **Fixed (auth-api LOW)**: L1 `Pagination` uses `saturating_add`; L2 archived projects refuse widget/key auth; L3 member add needs a visible, active user (no UUID oracle); L4 outbox per-job 45 s timeout, lease = batch x timeout + 60 s, crash loops dead-lettered (`dead_letter_exhausted`), a bookkeeping error no longer aborts the batch; L6 sign-up name rejects control chars; L7 attachment delete tolerates a missing file; L8 request timeout (120 s) and in-flight cap (512) layers; L10 `u32::try_from` for ticket numbers. L5 (SSRF ranges) was already in `dp_mail::guard`; L9 documented.
- **Fixed (mail-storage LOW)**: L1 executables refused by content, octet-stream resolved by extension; L3 `.tmp` sweep (housekeeping, 24 h); L4 0600/0700 modes; L5 directory fsync after rename; L7 percent-decoded `X-Filename`/`X-Message`; L9 startup check that file < request < upload/webhook limits, decoded webhook attachment size rechecked; L10 webhook `to` normalised (bounds already in ingest); L11 storage sanitiser trims after truncation; L12 `random_id` uses a v4 UUID; L6 as above. L2 (orphan GC) and L8 (bearer token URL) documented in `SECURITY.md` "Accepted risks"; L13 kept (tested helpers).
- **Lesson**: [one durable send path](/wisdom/time-traps.md): a reply sent inline and an auto-response queued had different failure semantics, and the inline one could mail before the message existed; queue in the same transaction as the row it describes.

## [2026-11-07] P3 deploy verify + ops

- **Verified natively (no docker)**: both Caddyfiles `caddy validate` + `caddy fmt` clean. Dockerfile steps replayed in a clean temp dir: `cargo build --release -p api --locked` (54 s, binary `api`), `bun install --frozen-lockfile` + `bun run build` with `NEXT_PUBLIC_API_URL=` `NEXT_PUBLIC_API_VERSION=v3` -> `out/index.html`, `widget/`, `_next/`; layout matches Dockerfile (`/usr/local/bin/api`, `/srv`), `deploy/web.Caddyfile` (`root * /srv`), host Caddyfile (`/srv/deskpost/web`). Release binary (DP_ENV=production, migrate, bootstrap, serve) behind the production Caddyfile (local TLS, only address/root changed): `scripts/smoke.sh` 100% green incl. 41 MB proxy cap (413), 64 KB api cap (413), CORS allow/deny, CSP/HSTS/nosniff/XFO, widget frameable.
- **Created**: `docs/OPERATIONS.md`, `scripts/smoke.sh` (re-runnable on a real host), `scripts/backup.sh` (pg_dump custom + files tar, retention, refuses client/server major mismatch), `scripts/deploy-check.sh` + `make deploy-check` (caddy, compose/.env.example/systemd/Dockerfile vs `config.rs`, backup->restore round trip).
- **Fixed (deploy only, no app code)**: `DP_FIRST_USER_ADMIN` was read by `config.rs` but missing from compose and `.env.example`; `.env.example` set `DP_ALLOW_SIGNUP=true` while code default is false, and `docs/API.md` claimed default true (both now false); systemd `EnvironmentFile` does not strip inline `# comments` (copying `.env.example` raw breaks `DP_ENV` etc.): documented `sed` strip; env file `STORAGE_DIR` overrides the unit's `Environment=` and the example's `./data/storage` is read-only under `ProtectSystem=strict`: documented.
- **Findings, not fixed**: api binds `0.0.0.0:$PORT` with no bind setting (firewall port 3000 on host installs); `DP_SECRET_KEY` rotation has no re-seal tool (mail passwords must be re-entered); `docker build` and compose still never run here (no docker); systemd unit only syntax-checked by parsing (no `systemd-analyze` on macOS). No app bug found.
- **Lesson**: [pg_dump newer than the server is not restorable](/wisdom/time-traps.md); the Homebrew default `pg_dump` here is 18 against a 16 server.

## [2026-11-07] R50 backend fixes

- **Creation**: SSRF check at connect time, IMAP timeouts and size-bounded fetch, ingest sanitising and sender check, signup off by default, client scope on tickets, status race, no reflected text in auto-responses, mailbox approval by a global admin (migration 0003), attachment mail after commit (same rule as `/messages`), streamed downloads, summed attachment size cap, per-account login throttle, bounded hashing queue.
- **Decision**: attachment upload with `sendEmail` no longer fails or rolls back when mail fails; it saves, then logs the failure like replies. A mailbox is approved for webhook routing by the platform operator (global admin); a tenant cannot prove an address, because a login or a mailed code both pass through a server the tenant picks.
- **Lesson**: [login to a chosen server proves nothing](/wisdom/time-traps.md).

## [2026-11-06] R50 fix web-deploy

- **Fixed**: H1 web image is now `caddy:2-alpine` serving the static export (`deploy/web.Caddyfile`, proxies `/api` to `app:3000`, same origin); host `deploy/Caddyfile` serves `/srv/deskpost/web` directly. H2 one name `NEXT_PUBLIC_API_URL` (empty = same origin) in Dockerfile, compose, `.env.example`. H3 no localStorage tokens; access token in memory, session restored via `/auth/refresh` cookie (demo-mock restore gated on `NEXT_PUBLIC_USE_MOCK`). M1 `lib/debug.ts` no-op in prod replaces all `console.log`. M2 typed `AuthExpiredError`, match 401 + TOKEN_* codes only, `ApiRequestError` carries `details`. M3 single-flight + `navigator.locks` refresh. M4 CSP Report-Only + Permissions-Policy. M5 widget `Referrer-Policy: no-referrer`. M6 UI types = server list, contract test `e2e/tests/attachment-types.spec.ts`. M7 blob URL revoke + stale guard. L1 jar sha256 pin, L2 e2e refuses non-local db (`E2E_DATABASE_URL`), L3 trap INT/TERM + child kill, L5 compose env allow-list, mailpit profile `dev`, `.dockerignore`, L6 proxy health/timeouts.
- **Not done / notes**: M5 per-project frame-ancestors needs API change (crates untouched). M6: server serves PDF download-only (`content_type.rs`), so viewer "unknown" for PDF is correct; review claim wrong. Systemd hardening extras, image digest pins, API `healthcheck` subcommand (no curl) and 40MB body split in Caddy left. 422 field `details` now reach `ApiRequestError.details`, forms do not render them yet. Docker not installed: image and compose not built; Caddyfiles validated with `caddy validate`.
- **Lesson**: `next start` + `output: export` fails at boot and e2e never touches the Docker target; add a docker smoke when docker is available.
- **Verify**: `make web` ok, `make e2e` 6/6 pass.

## [2026-11-06] R40 e2e

- **Creation**: `scripts/e2e.sh` (scratch db `dp_e2e` on :5433, GreenMail 2.1.15 jar in `.tools/`, api binary with migrate+bootstrap, Caddy serving `apps/web/out` and proxying `/api` + `/health`, then Playwright) and `e2e/` (`@playwright/test` on system Chrome, imapflow, nodemailer). Flows: bad/good login, create project in UI, widget ticket, agent reply -> mail out, customer email -> IMAP poll -> ticket -> reply -> mail out.
- **Decision**: ports api 3100, web 4100, SMTP 3025, IMAP 3143 (script refuses busy ports). Mail project is created via API because the UI create-project dialog has no IMAP fields (gap, not a bug in scope).
- **Lesson**: reply mail subject is `Re: <project> Support - [Ticket #N]`, not the customer subject; match on the ticket number. No api/web bug surfaced by these flows.

## [2026-11-06] R31 deploy docs

- **Creation**: multi-stage `Dockerfile` (targets `api`, `web`), `docker-compose.yml` (postgres, migrate, app, web, mailpit), `deploy/Caddyfile`, `deploy/deskpost-api.service`, `.env.example` (every var in `config.rs` and `mail_config.rs`), `SECURITY.md`, new `README.md` linking the free-email guide.
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
- **Decision**: status changes go through the dp-core machine (closed only reopens to pending, same status is a no-op), so API.md 7.3 "any to any" was corrected. Mail is sent after commit and failures are recorded, replacing "mail first, 502" of API.md 7.4 (R23 may revisit). Delete stays a hard delete (API.md deviation 16), not a soft delete.
- **Lesson**: [axum multipart and query rejections](/wisdom/time-traps.md).
## [2026-10-27] R21 api projects

- **Creation**: routes for projects, members, users, clients, templates, email responses and SMTP/IMAP settings with probes ([architecture](/architecture/projects-api.md)); `mail_guard` SSRF policy; `dp_mail::check_smtp`; db `update_with_mail`, `admin_update_guarded`, `delete_guarded`, `email_responses::project_of`.
- **Decision**: TLS derived from port (no schema change); passwords sealed with project+field AAD; `POST /users/me/password` and read-only `/clients` added to `docs/API.md`.
- **Lesson**: [two-step writes need one transaction](/wisdom/time-traps.md).

## [2026-10-27] R20 api auth

- **Creation**: `crates/api` (lib + `api` binary: serve, migrate, bootstrap), config, envelope/errors, HS256 JWT, Argon2id hasher, sliding-window limiter, trusted-proxy client IP, auth routes, extractors, health; see [API crate](/architecture/api.md). 100+ tests, including a reusable tenant fixture.
- **Decision**: `/auth/config` kept (reports `signupEnabled`), `/health` split into liveness and `/health/db`, default body cap 64 KiB, new `METHOD_NOT_ALLOWED` code; all recorded in `docs/API.md`.
- **Lesson**: [refresh grace must not clear the cookie](/wisdom/time-traps.md).

## [2026-10-27] R12 storage

- **Creation**: `dp-storage` (Storage trait, FsStorage, DownloadSigner, check_quota), [architecture page](/architecture/storage.md) with TS weaknesses. S3 driver deferred.
## [2026-10-27] R11 dp-core

- **Creation**: `crates/core` modules status, email_addr, threading, body, secret, api_key, filename, content_type, `constant_time_eq`; see [dp-core](/architecture/core.md). All `emailParser.test.ts` cases (and the extractor cases 1-20) rewritten as Rust tests plus proptest properties; 148 tests.
- **Decision**: TS bugs fixed instead of ported (RFC 2047 spoof, `From:` over-cut, bounce prefix match, key derivation); PDF kept download-only.
## [2026-10-27] R13 mail crate

- **Creation**: `dp-mail` (lettre sender, mail-parser inbound, async-imap `Mailbox`/`poll`, `testing` fakes), `docs/free-email.md`, [architecture/mail.md](/architecture/mail.md).
- **Lesson**: [mail-parser and IMAP gotchas](/wisdom/time-traps.md).
## [2026-10-27] R10 db + contract

- **Creation**: `docs/API.md`, `crates/db` (dp-db) with migration 0001, 13 query modules, 69 DB tests. See [DB crate and API contract](/architecture/db-contract.md).
- **Decision**: templates and clients are per project; API key hashed; `/clients` CRUD dropped; envelope `{success,...}` means R30 must change UI auth `status` checks.

## [2026-10-27] R00 scaffold

- **Creation**: Rust workspace (dp-core, dp-db, dp-mail, dp-storage, api), Makefile, CI, deny.toml, `docs/PLAN.md`, brain. Next.js demo moved to `apps/web`.
