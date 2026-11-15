# Review: mail + storage (R50)

Scope: `crates/mail`, `crates/storage`, `crates/api/src/{mail_out,ingest,mail_config,workers}.rs`, `routes/{incoming,attachments}.rs` (plus the `dp-db` queries and `mail_guard` they call). Read-only; no source edited, no tests run (static review). Docs checked: `docs/API.md` sections 9 and 12.

Checked and fine: path traversal in storage (strict key shape, symlink checks, generated keys), download-token HMAC (length-prefixed, constant time, key-bound), webhook HMAC (constant time, signature before freshness), header injection in outbound mail (`no_control` everywhere), inline-serving allowlist (no SVG/HTML/PDF, `nosniff`), per-tenant Message-ID dedupe (`UNIQUE (project_id, email_message_id)`), quota race (project row `FOR UPDATE`), tenant scoping of attachment queries.

## High

### H1. One inbound mail can wedge a project's IMAP inbox forever (poison pill)
- `crates/api/src/ingest.rs:249` passes `email.from_name` to `create_inbound_ticket` untruncated, and `ingest.rs:221` does the same for `sender_name`. `clients.name` has `CHECK (length(name) <= 255)` (`0001_init.sql:106`). `clients::get_or_create` (`crates/db/src/clients.rs:28`) binds it as-is. Postgres checks the proposed row before `ON CONFLICT`, so this fails even for an existing client.
- `crates/api/src/workers.rs:73` turns any `ingest` error into `return Err(...)` **before** `set_imap_cursor` (`workers.rs:82`). The cursor never advances, so every 60 s tick refetches the same round and fails on the same mail.
- Failure scenario: anyone emails the support address with `From: "<300 chars>" <a@b.com>`. Every later mail for that tenant is never ingested until someone edits the mailbox by hand (no admin UI for it). The only symptom is `imap_last_error`.
- Same class, other inputs: NUL bytes in body, subject or names (Postgres `text` rejects `\0`; `clean_subject` strips control chars but `body_of` and `email_subject: Some(email.subject.clone())` do not), and any oversized `email_*` value.
- Fix:
  - Sanitise and truncate everything stored: names to 255 chars, strip `\0` and control chars, cap `email_subject`, `email_from`, `email_to`, `email_references`.
  - In `poll_project`, treat a per-message *data* error as "consume and log", like parse errors on `workers.rs:76`. Only abort on connection-level DB errors. Distinguish `sqlx::Error::Database` constraint/22021 from pool/IO errors.
  - Optionally add a bounded retry counter per UID.

### H2. IMAP poll can hang forever and stall every project's polling
- `crates/mail/src/imap.rs:328-334`: `client.read_response()` for the greeting has no timeout (only TCP connect, STARTTLS, login and select are wrapped). A server that accepts the TCP connection and never sends `* OK` hangs `ImapMailbox::connect` for ever.
- `crates/api/src/workers.rs:99-116` (`poll_all`) awaits **all** spawned tasks and `every` uses `MissedTickBehavior::Delay` (`workers.rs:140`). One hung project holds a permit and blocks the whole tick. With `DP_IMAP_CONCURRENCY` (default 4) hung projects, all IMAP polling in the deployment stops. Any project admin can configure such a host (a plain tarpit), so this is a cross-tenant DoS.
- Fix: wrap the greeting read in `timeout(io, ...)`. Add an overall per-project deadline in `poll_all` (`tokio::time::timeout(60s, poll_project)`) and don't let `poll_all` wait on stragglers.

### H3. SSRF guard is connect-time-free: DNS rebinding / later flips reach internal hosts
- `mail_guard::check_resolved` is only called for the settings probe (`routes/project_mail.rs:358`). Real traffic never re-checks: `workers.rs:44-52` → `ImapMailbox::connect` (`imap.rs:301-306`, `TcpStream::connect((host, port))`) and `mail_out.rs:208-230` → lettre (`outbound.rs:252-270`) resolve and dial whatever is stored.
- Failure scenario: admin saves `evil.example` (public A record at probe time, TTL 0), then flips the record to `169.254.169.254` or `10.x`. The poller/outbox then connects to internal services every 15–60 s with an attacker-chosen port. Server banners and errors land in `imap_last_error` / `email_logs.error` (visible to project admins), which turns this into a blind port scanner or banner leak. `DP_ALLOW_PRIVATE_MAIL_HOSTS` is also only checked at save time. The doc comment on `check_resolved` accepts the window, but the window here is unbounded, not "between check and connect".
- Fix: resolve once at connect time, reject blocked IPs, and connect to the resolved `SocketAddr` (keep the hostname for TLS SNI/cert name). Apply it in both `imap.rs` and `outbound.rs`, or pass a pre-resolved address in `ImapConfig`/`SmtpConfig`. Consider a port allowlist (25/465/587/143/993/2525).

## Medium

### M1. Contract drift + weak authz on ticket threading (docs promise a sender check that only one path has)
- `docs/API.md` §12: a reply is appended only "when the sender address equals the ticket client's email or an address already present on the ticket". In `ingest.rs:99-123`, `ticket_number` (webhook), Message-IDs we minted (`ticket-<uuid>@…` names the ticket directly) and `References` matches via `find_ticket_by_refs` (also matches `email_logs.smtp_message_id`) have **no** sender check. Only the weakest signal, the subject tag (`ingest.rs:126-132`), is checked.
- Failure scenario: anyone who holds one of our Message-IDs (forwarded mail, CC/BCC'd third party, leaked headers) sends `References: <ticket-uuid@domain>` and injects a message. It reopens a `resolved`/`closed` ticket to `in_progress` (`messages.rs:235`) and marks it unread. Inbound mail is also never authenticated (no SPF/DKIM/`Authentication-Results` check), so even the subject-tag check is defeated by spoofing the client's `From`.
- Fix: apply the sender check to every threading path (or open a new ticket on mismatch), as the doc says. Compare against client email plus addresses already on the ticket. For IMAP, require a passing `Authentication-Results` (or document the limit). Fix the doc if the webhook number is meant to be trusted, since the webhook is signed.

### M2. Attachment email is sent before the message is saved; failure after send leaves orphans and duplicates
- `routes/attachments.rs:214-231` (`save_message`): `send_reply` runs first, then `post_agent`. If `post_agent` fails (`Posted::BadAttachments`, DB error), `post_files` (`attachments.rs:180-189`) deletes the attachments and files, but the client already got the mail with the files. Retrying sends it again. The same happens in `mail_out.rs:328`: if `email_logs::mark_sent(...)?` fails after SMTP success, the caller sees an error and the user retries (duplicate mail).
- Docs/R22 say replies are sent **after commit** and a mail failure never fails the request (`routes/tickets/messages.rs:83-86` does that). The attachments route instead returns 502 on mail failure and saves nothing, which is inconsistent behaviour for the same operation.
- Fix: reuse the messages route flow (commit message with attachments, then `deliver`), or at minimum treat a post-send DB failure as "sent" and don't roll back. Make `mark_sent` failure non-fatal in `send_reply`.

### M3. Unbounded outbound mail size and memory (limits bypassed by linking many uploads)
- Upload caps (10 MB/file, 25 MB/request) apply per upload request. `POST /tickets/:id/messages` accepts up to 20 `attachmentIds` of earlier uploads (`routes/tickets/messages.rs:66`) and nothing sums sizes. `mail_out.rs:237-277` reads all of them into memory, and `outbound.rs:215` clones each (`a.bytes.clone()`), then lettre base64-encodes: 20×10 MB ≈ 200 MB → ~600 MB+ peak per request. Real SMTP servers also reject such mail, so it retries as a 502/failed log. Concurrent requests multiply it.
- Fix: enforce `max_request_bytes` over the summed size in `post_agent`/before `send_reply` (422), and avoid the `.clone()`.

### M4. Auto-response relays attacker-controlled text to arbitrary addresses
- `mail_out.rs:450-458` appends 300 chars of the ticket description to the auto-reply, sent to `client_email`. For the public widget (`X-Project-Key`) and spoofed inbound mail, both the recipient and the text are attacker-chosen. The project's SMTP identity becomes a (rate-limited) spam/phishing relay and backscatter source. Bound: 5/hour/address/project, 1000 pending (`mailq.rs:256-258`); an attacker can still use many addresses. The comment on `mail_out.rs:452` acknowledges the risk but the cap does not remove it.
- Ticket creation itself has no per-sender or per-IP limit on the IMAP path (ticket flood, unread flood, attachment quota burn).
- Fix: drop the excerpt (or send only the ticket number/subject the project controls); add a per-sender ticket-creation throttle; optionally require double-opt-in for widget-created tickets.

### M5. Inbound attachments can be silently and permanently lost
- `ingest.rs:237` / `264-271`: the message commits first, then `store_attachments` runs. Failures (storage unavailable `143-146`, put error `156-159`, quota `172-175`) are only logged. On a crash between commit and storing, or any of those errors, a replay returns `Outcome::Duplicate` (same Message-ID), so the attachments are never recovered. Quota rejection also drops the mail's files with no trace on the message.
- Fix: store files first (orphan-collectable), then insert message + attachment rows in one transaction; or record a "N attachments dropped" note on the message. Return an error (and don't advance the cursor) when storage is unavailable.

### M6. Webhook without Message-ID dedupes distinct mail
- `routes/incoming.rs:160-182`: the fallback id hashes from/to/subject/body only (no date or provider id). Two legitimate identical mails ("Thanks", same subject) from one sender are the same id, so the second is dropped as `duplicate: true` for ever (unique per project). The IMAP fallback hashes the raw bytes (includes Date), which is better.
- Fix: include `date`/a provider event id in the digest, or require `messageId` on the webhook (422).

### M7. Download endpoint loads the whole file in memory, no range/length handling
- `routes/attachments.rs:406-412` reads the file into a `Vec` (≤ `max_file_bytes`, configurable to 512 MB via `DP_MAX_FILE_BYTES`, `mail_config.rs:56-62`) per request. `take(size_bytes+1)` also silently serves a file that grew/was replaced on disk. A few concurrent downloads of large files cost GBs.
- Fix: stream (`ReaderStream` + `Body::from_stream`), set `Content-Length` from `size_bytes`, error if the file length differs.

### M8. IMAP fetch buffers whole batches before any size check
- `imap.rs:381-407`: `UID FETCH <50 uids> BODY.PEEK[]` collects every full message into `Vec<RawMessage>` and only then `parse_inbound` checks `max_message_bytes` (`inbound.rs:80`). A sender can queue 50 mails near the server's limit; × `imap_concurrency` projects that is multi-GB. The 30 s `io_timeout` wraps the entire batch download (`imap.rs:394`), so a slow link or big backlog always times out. The cursor never advances, which is the same wedge as H1.
- Fix: `FETCH RFC822.SIZE` first, skip oversize UIDs (consume + log), fetch in small byte-bounded groups, scale the timeout, or stream per message.

## Low

- **L1. Declared MIME is trusted.** `attachments.rs:41-63,102` allowlists on the client-supplied `Content-Type`; there is no magic-byte check. An `.exe` sent as `application/pdf`/`application/zip` is stored and then emailed to the client (malware relay, reputation risk). Inbound mail/webhook attachments (`ingest.rs:136-176`) have no allowlist at all (they are served download-only, so no XSS). Also the 415 for `application/octet-stream` PDFs from some clients/curl, and an empty multipart part type (`""`), is surprising. Fix: sniff with the `infer` crate and cross-check; allow octet-stream + extension.
- **L2. Delete leaves orphan files / ghost rows.** `attachments.rs:462-472`: the DB row is deleted first, then `fs.delete`; if the file removal fails the row is gone (quota freed) and the file is unreachable and unreferenced, with a 500 to the user. `post_files` cleanup (`183-187`) ignores errors the same way, and a crash mid-upload leaves files (`put` stores the file before the DB row). There is no orphan GC. Fix: periodic sweep comparing store to `attachments.storage_key`; or delete the file first, then the row (idempotent delete).
- **L3. `.tmp` leftovers are never collected.** `fs.rs:148-160`: `TempGuard` cleans on drop, but a process kill/crash leaves `<root>/.tmp/*` forever. Fix: sweep `.tmp` older than N hours at startup.
- **L4. File permissions.** `fs.rs:108-112,152` use default modes (0644 files / 0755 dirs under the umask), so other local users can read attachments. Fix: `mode(0o600)` / `0o700` (`OpenOptionsExt`, `DirBuilder`).
- **L5. No directory fsync after rename** (`fs.rs:173-178`): a power loss can lose a file the DB row points to. Fix: fsync the parent directory.
- **L6. Outbox lease vs batch time.** `mail_out.rs:395-410`: 20 jobs are leased for 300 s but sent sequentially (SMTP total timeout 60 s each = up to 20 min). With >1 API instance, later jobs' leases expire and are re-claimed (double send). An early `?` on `mark_sent`/`mark_failed` abandons the rest of the batch with burned attempts (`attempts` is incremented at claim). Fix: extend the lease per job, or send with bounded concurrency and don't `?`-abort the loop.
- **L7. Non-ASCII `X-Filename` / `X-Message` are silently dropped** (`attachments.rs:238-262`: `to_str()` fails on non-ASCII, so the file becomes `file` and the message is lost). The UI should percent-encode and the server decode (document it).
- **L8. Signed download token** (`attachments.rs:440-446`) is a bearer capability in the URL: not bound to the user, not revocable when membership is removed (valid up to 5 min plus 30 s leeway; `download_ttl` is not configurable, `mail_config.rs:82`). It will appear in access logs. Acceptable, but log redaction of `token=` should be ensured.
- **L9. Config drift.** `DP_UPLOAD_LIMIT_BYTES` (26 MB, `config.rs:185`) and `DP_WEBHOOK_LIMIT_BYTES` (36 MB, `config.rs:193`) are independent of `DP_MAX_REQUEST_BYTES` (up to 512 MB, `mail_config.rs:64`). Raising one without the others yields a 413 before the intended cap, and `max_file_bytes > max_request_bytes` is accepted. Fix: derive or validate at startup. The webhook base64 pre-check (`incoming.rs:126-131`) estimates size from text length and never rechecks decoded size.
- **L10. Webhook `to` stored raw** (`incoming.rs:207`: `to: vec![p.to]` before normalisation; also unbounded `subject`/`references` flow into `email_subject`/`email_references`). The sender is trusted (signed) but this is the same unbounded-column class as H1.
- **L11. Duplicate filename sanitisers.** `dp_core::filename::sanitize_filename` (API) and `dp_storage::sanitize_filename` differ (the storage one is char-set based and can end in a trailing dot/space after truncation, `name.rs:21-26`). Behaviour is safe today but they can drift; keep one.
- **L12. `random_id()`** in `outbound.rs:146-152` uses `RandomState` (SipHash keys, not a CSPRNG) for the fallback Message-ID. Callers here always pass an id, so it is dormant; use `Uuid::new_v4()`.
- **L13. Unused code.** `dp_storage::check_quota` and `Storage::exists` are not used by the API (quota is done in SQL); keep tests or drop.

## Contract drift vs docs/API.md

| Doc says | Code does |
|---|---|
| §9: on-disk key `{projectId}/{ticketId}/{uuid}` | `fs.rs:144-145`: `ab/cd/<uuid>` fan-out, no project/ticket in the path; DB `storage_key` is opaque |
| §9: `Content-Disposition: inline` for png, jpeg, gif, webp, **pdf**, plain text | `content_type.rs:11-20`: pdf is download-only; avif, bmp, csv are inline |
| §9 endpoint table | missing `GET /attachments/file/:id/url` (signed link) and `DELETE /attachments/file/:id` |
| §9: allowed types list (images, PDF, text, office, zip) | also `image/avif`, `image/bmp`, `text/csv`, OpenDocument; SVG explicitly excluded |
| §12: sender must match the ticket client/ticket address on `ticketNumber` **or** threading match | only the subject-tag path checks it (M1) |
| §12 body fields | code also accepts `autoSubmitted` and `precedence` (undocumented); `201` for new/reply, `200 {duplicate\|ignored}` otherwise |
| §7 messages: mail failure never fails the request | the attachments upload route returns 502 and saves nothing (M2) |

## Suggested order
1. H1 + M8 (same wedge; cheap fix: sanitise/truncate and consume-on-data-error).
2. H2 (one timeout line plus a per-project deadline).
3. H3 (resolve-at-connect guard shared by `dp-mail`).
4. M1, M2, M3, then the rest.
