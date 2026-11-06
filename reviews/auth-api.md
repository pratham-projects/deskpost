# R50 review: auth + API (crates/api, crates/db, crates/core)

Read-only review. No source edited. Scope: auth, extractors, config, projects, members, tickets,
messages, attachments, incoming mail, mail guard, workers, db queries, core helpers, against `docs/API.md`.
Line numbers are from the tree at `feat/review` (2eec432a). Nothing was run; every finding is from reading code.

Verdict on the good parts: tenant isolation in SQL is solid (actor-scoped queries, composite FKs, 404 on non-members),
JWT/refresh rotation/reuse detection, Argon2 dummy-hash, rate limiter shape and filename/content-type handling all look right.
Findings below are what is left.

## HIGH

### H1. Stored SMTP/IMAP hosts are dialed with no SSRF check (blind SSRF + internal port scan)
- `crates/api/src/workers.rs:44-52` (IMAP poller connects to `imap_host`), `crates/api/src/mail_out.rs:223,325,468` (SMTP send to `smtp_host`).
- `crates/api/src/routes/mail_guard.rs:59` `check_host` is syntax/static only (no DNS). `check_resolved` (line 98) is called **only** from the probe endpoints (`project_mail.rs:358`). Nothing in `crates/mail/src` blocks private addresses.
- Failure: any signed-in user (sign-up is open by default, any user may `POST /projects`) saves `imapHost: "x.attacker.com"` where the name resolves to `127.0.0.1`/`10.x`/`169.254.169.254`. `check_host` passes (it is a public-looking name). Every poll tick (`imap_poll_interval`) the server dials the internal address and port of the attacker's choice, and the outcome is stored in `projects.imap_last_error`. The same via `smtpHost` on the first auto-response/reply. DNS rebinding makes it worse (resolve public at save, private at dial). `docs/API.md:190` says resolved addresses are checked, which is only true for the probe.
- Fix: resolve in the worker/mailer path and refuse blocked IPs (`is_blocked_ip`) unless `allow_private_mail_hosts`; connect to the resolved `SocketAddr` (pin the IP, keep SNI = hostname) so there is no TOCTOU. Put the guard inside `sd_mail` connect so every caller gets it. Also re-check on every poll, not only at save.

### H2. Mailbox ownership is never proven; first claimant of `smtpEmail` wins and can receive another tenant's mail
- `crates/api/src/routes/project_mail.rs:74-81` (`email()` only checks syntax), `projects.rs:204,274`, `crates/db/src/projects.rs:324` (`smtp_email` set directly), `projects.rs:368` (`find_by_smtp_email` routes inbound by it).
- `docs/API.md:339` states the mailbox "must be proven reachable by the admin's own SMTP/IMAP login before it is saved (R21)". Code does not do this.
- Failure: attacker creates a project with `smtpEmail: "support@victim.com"` (unique index makes the victim's own later attempt return 409 "belongs to another project"). With the signed webhook enabled (`SD_WEBHOOK_SECRET`, one global secret for all tenants) the provider posts `to: support@victim.com`; `find_by_smtp_email` returns the attacker's project, so the victim's customer mail is turned into tickets the attacker can read, and the victim is locked out. Without the webhook only squatting/DoS applies.
- Fix: do not persist `smtp_email` until a successful probe (SMTP auth + IMAP login) with that address succeeded in the same request, or require a verification mail/token round trip. Mark unverified claims inactive for routing.

### H3. First sign-up becomes global admin, and sign-up is on by default
- `crates/db/src/users.rs:77-94` (`CASE WHEN EXISTS (SELECT 1 FROM users) THEN 'agent' ELSE 'admin'`), `crates/api/src/config.rs:201` (`SD_ALLOW_SIGNUP` default `true`), `routes/auth.rs:86`.
- Failure: a fresh deploy that is reachable before the operator runs `bootstrap` hands full user-management (`/users` list/create/patch/delete, `GlobalAdmin`) to whoever signs up first. Also, with sign-up open, every stranger can create projects and attach mail hosts (feeds H1/H2).
- Fix: default `SD_ALLOW_SIGNUP=false` in production; drop the "first user is admin" rule (the `bootstrap` command already exists) or only honour it when an explicit `SD_FIRST_USER_ADMIN=1` is set. Consider requiring global-admin (or an allow flag) for `POST /projects`.

## MEDIUM

### M1. Contract drift: `resolved` auto-response never sent; `created` auto-response only for inbound mail; `shouldNotify` does nothing
- `docs/API.md:242` ("Entering `resolved` ... sends the `resolved` email response (if enabled) after commit") and `:216` (confirmation mail for created tickets).
- `crates/api/src/routes/tickets/read.rs:180-182` ignores `TicketUpdate.previous_status` and never calls `enqueue_auto_response`; its only caller is `ingest.rs:272` with `Created`. Widget/admin tickets (`create.rs:128,151`) enqueue nothing. `members::notify_recipients` (`db/src/members.rs:167`) has no caller, so `should_notify` is dead. `mailq::imap_status` (`db/src/mailq.rs:411`) has no caller, so `imap_last_error` is never shown.
- Failure: admins configure the "resolved" response and clients never get it.
- Fix: in `read::update`, when `previous_status != new` and new == `Resolved`, call `enqueue_auto_response(.., Resolved)` after the update returns; same for `Created` in `create::widget/admin` (honouring `sendNotification`); wire `notify_recipients`. When you add the widget path, keep the bounds in `mailq::enqueue` and consider a per-IP/per-project cap for unverified `clientEmail` (see M8).

### M2. Threading by webhook `ticketNumber` and by References/Message-ID skips the sender check the docs promise
- `crates/api/src/ingest.rs:99-123` (explicit number and Message-ID paths return the ticket with no sender comparison; only the subject-tag path at 126-131 checks `client_email`). `docs/API.md:341` says the sender must equal the ticket client's (or an address already on the ticket) for `ticketNumber` and threading matches.
- Failure: anyone who learns a ticket's minted Message-ID (CC'd third party, forwarded mail) or any webhook sender with a guessed number can append messages to, reopen (`post_inbound` flips `closed`/`resolved` to `in_progress`, `db/src/messages.rs:236`) and mark unread any ticket in that project. `From` is also unauthenticated (no SPF/DKIM/DMARC result is consulted), so the sender check is only a speed bump; at minimum make all three paths consistent.
- Fix: apply the same "sender is the client or already on the thread" check to every path before `post_inbound`; otherwise open a new ticket. Consider honouring provider auth results (`Authentication-Results`) before trusting `From`.

### M3. Deleting a ticket leaves its attachment files on disk
- `crates/api/src/routes/tickets/read.rs:195-207` calls `tickets::delete` (cascades attachment rows) and never removes files; `db::attachments::storage_keys_for_ticket` (`db/src/attachments.rs:174`) exists and is unused. Project delete does clean up (`routes/projects.rs:312-326`), so only tickets leak.
- Failure: customer files persist forever after a delete (retention/privacy problem); disk grows without a quota row to account for it.
- Fix: fetch keys first (as `projects::remove` does), delete rows, then best-effort delete files.

### M4. `GET /clients` ignores `can_view_all_tickets`
- `crates/api/src/routes/clients.rs:50-64`, `crates/db/src/clients.rs:57-75,46-54`: membership is the only check.
- Failure: a member restricted to their own tickets (`can_view_all_tickets=false`) lists every client's email, name and phone in the project, defeating the restriction (`tickets::VISIBLE`).
- Fix: limit restricted members to clients of tickets they can see (join on `VISIBLE`) or deny the endpoint to them.

### M5. Ticket status validation races with the update
- `crates/api/src/routes/tickets/read.rs:165-180` reads `current` outside the transaction, validates the transition, then `db::tickets::update` (`db/src/tickets.rs:364-395`) locks the row but applies `status` unconditionally and discards `previous_status`.
- Failure: two concurrent PATCHes (or an inbound mail reopen) race: ticket is `closed` in the DB while the handler saw `resolved`; the handler then writes `in_progress`, violating "closed can only be reopened to pending". `Unchanged` detection has the same gap.
- Fix: do the transition check in `tickets::update` after `FOR UPDATE` (pass `core::transition`), and return `409` from there.

### M6. Rate limiting is per IP only and misconfigures easily
- `crates/api/src/ratelimit.rs`, `routes/auth.rs:140`, `client_ip.rs:15`.
- (a) No per-account throttle: 10 logins/min per IP means a botnet/IPv6 pool guesses one account's password without limit. (b) With `SD_TRUSTED_PROXIES` unset behind a reverse proxy, every client shares the proxy's IP: 10 sign-ins/min for the whole site, and one attacker locks everybody out (`Rule::Login`), same for `Register`/`Refresh`. Nothing warns at startup. (c) When the table is full (`ratelimit.rs:66-79`) the least-recent key is evicted, so an attacker spraying 10k distinct keys (IPv6 /64s, or spoofed XFF if the peer is trusted) resets other clients' counters. (d) `Hasher` semaphore (`password.rs:35,45`) queues unauthenticated sign-in/up requests without bound, so a flood holds request tasks and memory.
- Fix: add a per-email (lower-cased) failure counter with backoff; log a loud warning (or refuse to start in production) when the peer is a private address and no trusted proxies are configured; evict by oldest window instead of LRU or make eviction fail-closed; bound the hasher queue (`try_acquire` + 503).

### M7. Large files fully buffered in memory
- `crates/api/src/mail_out.rs:259-269` (`send_reply` loads every attachment: up to 20 x `max_file_bytes` into RAM per reply), `routes/attachments.rs:260,300,406` (`body.to_vec()` copies the upload; downloads read the whole file into a `Vec`).
- Failure: a few concurrent authenticated requests with big attachments exhaust memory (no concurrency limit, no timeout layer in `lib.rs:90-112`).
- Fix: stream downloads (`Body::from_stream`), avoid the `to_vec` copy (`Bytes` -> store), cap total attachment bytes per mail, add a concurrency limit and request timeout layer.

### M8. Reply email is sent inline, not durably, and the two reply paths disagree
- `routes/tickets/messages.rs:89-141`: the message is saved, then SMTP runs inside the request (up to 30 s held) with no retry, whereas auto-responses use the outbox. `routes/attachments.rs:214-230` (`save_message`) sends the mail **before** `post_agent`, so if saving fails the client already got the mail and the files are rolled back (ghost email); and it fails the whole request on mail errors (502) while `/messages` swallows them (201). `mail_out.rs:131` comment ("Sent before the message is saved") contradicts `docs/API.md:254`.
- Fix: send replies through the outbox like auto-responses (one code path, retries, no request-time SMTP); pick one failure semantics and document it.

## LOW

- L1. Pagination overflow: `error.rs:171` `offset + limit < total` with `offset=i64::MAX` (accepted by `qparams.rs:52`) panics in debug / wraps in release. Use `saturating_add`.
- L2. Archived projects still accept widget tickets and key auth: `db/tickets.rs:147-154` and `projects.rs:359` check `status='active'` only; `is_archived` is honoured only for inbound mail and IMAP. Decide intended behaviour and check it in both.
- L3. `POST /projects/:id/members` (`routes/members.rs:114-134`): a project admin (any user who created a project) can probe arbitrary user UUIDs (404 vs 201) and read the target's email/name in the response, and can add `inactive`/`suspended` users. Restrict to users visible to the actor (`users::get_visible`) and `status='active'`.
- L4. Outbox lease/poison: `mail_out.rs:27,396` processes 20 jobs sequentially with a 300 s lease; slow SMTP can exceed it and a second instance resends. `claim_due` (`db/mailq.rs:251`) bumps `attempts` but never dead-letters on a crash loop (the `MAX_ATTEMPTS` check only runs in `mark_failed`). Add a per-job timeout and a `attempts >= MAX` filter.
- L5. SSRF blocklist gaps (`mail_guard.rs:10-26`): no 6to4 `2002::/16`, Teredo `2001::/32`, `64:ff9b:1::/48`, `192.88.99.0/24`. Low once H1 is fixed, but cheap to add. Probe still has DNS-rebinding TOCTOU (documented); pin the IP as in H1.
- L6. `routes/auth.rs:72-75` `validate_account` accepts control characters in `name` (users.rs rejects them); sign-up returns 409 for existing emails (enumeration, limited by 5/h/IP). Reuse the `name()` validator; consider a generic response.
- L7. `projects::remove` (`routes/projects.rs:312-326`) reads storage keys before the delete; a concurrent upload orphans a file. `attachments::remove` (`:467-472`) returns 500 after the row is gone if the file delete fails (e.g. already missing). Treat `NotFound` as success.
- L8. `/health/db` (`lib.rs:91`) is unauthenticated and unthrottled; no request timeout/connection cap layer (slowloris). Add `TimeoutLayer` and a limit on concurrent connections.
- L9. Dev mode (`SD_ENV=development`) returns the refresh token in the JSON body (`auth.rs:261`) and uses SameSite=Lax; fine, but make sure compose/deploy files never set it in production (`SD_ENV` defaults to production, good).
- L10. `ticket_subject(.., ticket_number as u32, ..)` (`mail_out.rs:301,358,437`) truncates numbers above `u32::MAX`; harmless today, use `u32::try_from`.

## Checked and fine (no action)
JWT (fixed header, constant-time MAC, exp/nbf, 1 KiB cap); refresh families (hash at rest, FOR UPDATE, grace window, reuse revokes, user-status re-check); last-admin advisory locks; tenant scoping and 404-not-403 on non-members; composite FKs; LIKE escaping; filename sanitiser and content-type allowlist (`nosniff`, CSP `default-src 'none'`, download-only for active types); signed download tokens; AES-GCM with project/field AAD; secrets redacted from errors; webhook HMAC with timestamp window; API-key SHA-256 (256-bit random).
