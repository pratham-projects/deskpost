# Review findings status (R50 audit, re-checked against the current tree)

Reports: `auth-api.md` (A), `mail-storage.md` (M), `web-deploy.md` (W). Ids are `<report>-<finding>`.
Status: **fixed** (evidence cited), **accepted** (listed in `SECURITY.md` accepted risks), **open** (not fixed,
reason given), **n/a** (the finding was wrong). Paths are repo-relative, line numbers from this tree.

Fixed in this pass: A-M1 `shouldNotify` mail (new-ticket notice to flagged members). Everything else marked fixed was
already fixed on earlier branches (R50 fix commits, R51 leftovers).

## auth-api

| id | status | evidence |
|---|---|---|
| A-H1 SSRF on stored hosts | fixed | `crates/mail/src/guard.rs:69` `resolve` (resolve, reject blocked IPs, pin `SocketAddr`); used at connect in `crates/mail/src/imap.rs:321` and `crates/mail/src/outbound.rs:255`, so every poll and send re-checks |
| A-H2 mailbox ownership | fixed (by operator approval) | `crates/db/src/projects.rs:371` routes inbound only to `mail_verified_at IS NOT NULL`; `crates/api/src/routes/project_mail.rs:46,58` global-admin approve bound to the reviewed address |
| A-H3 first sign-up is admin / signup default on | fixed | `crates/api/src/config.rs:205-206` both default `false`; `crates/db/src/users.rs:77-98` first-admin only on opt-in |
| A-M1 contract drift (resolved/created mail, shouldNotify, imap error) | fixed, one nit open | resolved: `routes/tickets/read.rs:189-197`; created: `routes/tickets/create.rs:165,193` and `ingest.rs:337`; **shouldNotify now sends** (this pass): `mail_out.rs:459` `enqueue_team_notice`, `mail_out.rs:568` `send_notice_job`, `db/src/mailq.rs` `enqueue_team_notice`, migration `0007_team_notice.sql`, call sites `create.rs:163,191`, `ingest.rs:337`, tests `api/tests/mail_out.rs` (flagged/active only, pending bound). Open nit: `mailq::imap_status` still has no reader, so `imap_last_error` is written (`workers.rs:127`) but no endpoint returns it (not in `docs/API.md`) |
| A-M2 threading skips sender check | fixed | `ingest.rs:117` `find_thread` gates number, minted Message-ID, References and subject tag on `mailq::sender_on_ticket` |
| A-M3 ticket delete leaves files | fixed | `routes/tickets/read.rs:230` reads `storage_keys_for_ticket` before delete |
| A-M4 `/clients` ignores `can_view_all_tickets` | fixed | `crates/db/src/clients.rs:18-22` restricted members only see clients of tickets they can see |
| A-M5 status validation race | fixed | `crates/db/src/tickets.rs:383-404` `update_guarded` (`FOR UPDATE`, expected-status check, `Stale`); retry loop `read.rs:204` |
| A-M6 rate limiting | fixed | (a) per-account failure table `ratelimit.rs:58`; (b) startup warning `main.rs:66`; (c) fail-closed when full `ratelimit.rs:113`; (d) hasher wait 5 s then 429 `password.rs:34` |
| A-M7 large files buffered | fixed (one nit) | stream download `routes/attachments.rs:445-451`; summed size cap `mail_out.rs:317`; timeout + in-flight cap `lib.rs:118-122`. Nit: `outbound.rs:213` still clones each attachment once (bounded by the cap) |
| A-M8 reply sent inline | fixed | replies queued in the same transaction as the message: `routes/tickets/messages.rs:69`, `routes/attachments.rs:225` (one path, retries) |
| A-L1 pagination overflow | fixed | `error.rs:171` `saturating_add` |
| A-L2 archived projects accept widget/key | fixed | `crates/db/src/tickets.rs:149`, `projects.rs:361` `NOT is_archived` |
| A-L3 member add probes UUIDs | fixed | `routes/members.rs:115` `users::get_visible` (active only) |
| A-L4 outbox lease/poison | fixed | `mail_out.rs:487` per-job timeout, `:497` `dead_letter_exhausted`, lease = batch x timeout + 60 |
| A-L5 SSRF blocklist gaps | fixed | `crates/mail/src/guard.rs:30-52` covers 6to4, Teredo, NAT64 and mapped forms, with tests |
| A-L6 name control chars / enumeration | fixed / accepted | `routes/auth.rs:78` rejects control chars; 409 enumeration is in `SECURITY.md` accepted risks |
| A-L7 project delete race / attachment delete 500 | fixed / accepted | `routes/attachments.rs:507` treats `NotFound` as success; delete-vs-upload race in `SECURITY.md` |
| A-L8 `/health/db` unthrottled, no timeout | fixed / accepted | `lib.rs:118-122` timeout + concurrency cap; unauthenticated `/health/db` in `SECURITY.md` |
| A-L9 dev mode returns refresh token | accepted | `SECURITY.md` "Development mode" |
| A-L10 `ticket_number as u32` | fixed | `mail_out.rs:382` `u32::try_from` |

## mail-storage

| id | status | evidence |
|---|---|---|
| M-H1 poison-pill mail wedges IMAP | fixed | sanitise and cap: `ingest.rs:59` `clean_text`, `:294`; data errors consumed: `workers.rs:30` `is_data_error`, used in `poll_project` before the cursor moves |
| M-H2 IMAP greeting hangs, stalls all polling | fixed | `crates/mail/src/imap.rs:354` greeting under `timeout`; per-project deadline `workers.rs:23,123` |
| M-H3 SSRF only at probe | fixed | same as A-H1 (`guard.rs:69`, `imap.rs:321`, `outbound.rs:255`). Optional port allowlist not added (review said "consider") |
| M-M1 threading sender check | fixed | same as A-M2 (`ingest.rs:117`). `Authentication-Results` is still not consulted (From is unauthenticated); documented limit, not a bug |
| M-M2 attachment mail sent before save | fixed | `routes/attachments.rs:225` queues with the message; outbox sends after commit |
| M-M3 unbounded outbound size | fixed | `mail_out.rs:317` summed cap (clone nit as A-M7) |
| M-M4 auto-response relays attacker text | fixed in part, one open | description excerpt removed: auto-response body is the project's own text only (`mail_out.rs` `send_job`, `resp.body`); per-address bound `db/src/mailq.rs` (5/h, 1000 pending). **Open (hardening, not a bug)**: no per-sender or per-IP ticket-creation throttle on the IMAP/webhook path; not in `SECURITY.md`. Left for a product decision |
| M-M5 inbound attachments lost | fixed | store down fails before commit (`ingest.rs` storage check before create); per-file failures named in a note on the message `ingest.rs:170-225` |
| M-M6 webhook dedupe collides | fixed | `routes/incoming.rs:165-179` digest includes `date`/event id |
| M-M7 download buffered | fixed | `routes/attachments.rs:445-451` stream, `Content-Length` = recorded size, `take(size)` |
| M-M8 IMAP fetch buffers batches | fixed | `imap.rs:415` `RFC822.SIZE` first, byte-bounded groups |
| M-L1 declared MIME trusted | fixed / accepted | `routes/attachments.rs:107` `looks_executable`, octet-stream resolved by extension; "only sniffed for programs" in `SECURITY.md` |
| M-L2 orphan files | accepted | `SECURITY.md` "No orphan sweep"; attachment delete tolerates a missing file |
| M-L3 `.tmp` never collected | fixed | `workers.rs:152` `sweep_tmp` in housekeeping |
| M-L4 file modes | fixed | `crates/storage/src/fs.rs:25,47` 0700/0600 |
| M-L5 no dir fsync | fixed | `fs.rs:33` |
| M-L6 outbox lease | fixed | same as A-L4 |
| M-L7 non-ASCII `X-Filename`/`X-Message` | fixed | `routes/attachments.rs:257` `percent_decode` |
| M-L8 bearer download token | accepted | `SECURITY.md` "Signed download links are bearer URLs" |
| M-L9 config drift | fixed | `config.rs:216-222` startup checks; decoded webhook size rechecked |
| M-L10 webhook `to` raw | fixed | `routes/incoming.rs:214` normalised |
| M-L11 duplicate sanitisers | open (nit) | trailing-dot bug fixed in `storage/src/name.rs`; both functions still exist, the storage one is not called outside tests. No behaviour risk |
| M-L12 `random_id` not CSPRNG | fixed | `crates/mail/src/outbound.rs:149` `Uuid::new_v4` |
| M-L13 unused `check_quota`/`exists` | open (nit) | `storage/src/lib.rs:100` kept with tests on purpose (brain log R51) |
| Contract drift table (docs/API.md) | open (docs) | `docs/API.md:304` still says PDF is inline (code serves it download-only); `:307` still says on-disk key `{projectId}/{ticketId}/{uuid}` (code: `ab/cd/<uuid>`); endpoint table lacks `GET /attachments/file/:id/url` and `DELETE`; `autoSubmitted`/`precedence` undocumented. `docs/` is outside this task's scope; sender-check, outbox and 502 rows are now correct |

## web-deploy

| id | status | evidence |
|---|---|---|
| W-H1 web image cannot run | fixed | `Dockerfile:40-44` `caddy:2-alpine` serving `/srv`; `deploy/web.Caddyfile`; `deploy/Caddyfile` serves `/srv/deskpost/web`. Image never built here (no docker); CI has a docker job |
| W-H2 build arg name drift | fixed | `apps/web/lib/api/config.ts:5`, `Dockerfile:34-36`, `docker-compose.yml:98`, `.env.example:50` all `NEXT_PUBLIC_API_URL` |
| W-H3 tokens in localStorage | fixed | `apps/web/lib/store.ts:72` memory only, session restored via cookie; the only `localStorage` use is the demo build (`store.ts:56`, gated by `USE_MOCK` at `:109`) |
| W-M1 console logging | fixed | `apps/web/lib/debug.ts` no-op in production; no other `console.log` in `lib components app hooks` |
| W-M2 401 hangs / substring matching | fixed | `lib/api/client.ts:13-20,79,125` typed `AuthExpiredError`, `TOKEN_*` codes only |
| W-M3 refresh race | fixed | `lib/store.ts:76-109` single-flight + `navigator.locks` |
| W-M4 no CSP | fixed in part | `deploy/Caddyfile:30` CSP in Report-Only plus `frame-ancestors` and `Permissions-Policy`. Enforcing CSP is deliberately deferred ("promote once clean") |
| W-M5 widget frameable / key in URL | accepted by design | frameable on purpose (`deploy/Caddyfile` comment), `Referrer-Policy: no-referrer` added. **Open (docs)**: review asked for a line in `SECURITY.md`; none exists. `SECURITY.md` is outside this task's scope |
| W-M6 attachment type drift | fixed | `apps/web/lib/api/attachments.ts:15-29` subset of server list; `e2e/tests/attachment-types.spec.ts`. The "PDF viewer unknown" claim was wrong (server serves PDF download-only) |
| W-M7 blob URL leak | fixed | `components/attachment-viewer.tsx:62-93` local `objectUrl` revoked, `cancelled` guard |
| W-M8 blob content type constraint | fixed | note in `attachment-viewer.tsx:72`; no iframe/object preview |
| W-L1 jar without checksum | fixed | `scripts/e2e.sh:22,53,56` sha256 pin |
| W-L2 e2e drops db on any host | fixed | `scripts/e2e.sh:9-13` local hosts only, `E2E_DATABASE_URL` |
| W-L3 e2e orphan processes | fixed | `scripts/e2e.sh:36-37` trap EXIT/INT/TERM, child kill. The `nc` port-check race is accepted (dev only) |
| W-L4 run dir umask / curl healthcheck | fixed in part | `scripts/e2e.sh:28` `umask 077`. **Open (nit)**: `Dockerfile:24` `HEALTHCHECK` still uses `curl`; no `deskpost healthcheck` subcommand |
| W-L5 docker/compose hygiene | fixed in part | compose `environment:` allow-lists (`docker-compose.yml:9,23,58`), mailpit profile `dev` (`:105`), `.dockerignore` extended; systemd extras added this pass (`ProtectKernelModules`, `RestrictNamespaces`, `CapabilityBoundingSet=`, `UMask=0077` in `deploy/deskpost-api.service`). **Open (nit)**: base images not pinned by digest (cannot resolve digests without docker/network here); `SystemCallFilter` and `MemoryDenyWriteExecute` not added (cannot test a unit on macOS) |
| W-L6 Caddy body cap / timeouts | fixed in part | `health_uri` and transport timeouts in `deploy/Caddyfile`. **Open (low)**: one `max_size 40MB` for all `/api/*` (`Caddyfile:13`); the API enforces the real per-route limits. Splitting needs the upload and webhook paths to stay in step with `smoke.sh`, so left |
| W-L7 dead/misleading client code | fixed | `ApiError` type gone; 422 `details` kept (`client.ts:128`); `/api/${API_VERSION}` paths are the `apiClient` convention (it prepends the base), `API_PREFIX` is for direct `fetch` |

## Still open after this pass

1. A-M1 nit: `imap_last_error` is stored but no API field shows it.
2. M-M4: no per-sender ticket-creation throttle on the IMAP/webhook path (hardening, not in `SECURITY.md`).
3. Docs drift (outside scope): `docs/API.md:304,307`, endpoint table; `SECURITY.md` widget note (W-M5).
4. Nits: W-L4 curl healthcheck, W-L5 digest pins and extra systemd directives, W-L6 body cap split, M-L11, M-L13, outbound attachment clone.
