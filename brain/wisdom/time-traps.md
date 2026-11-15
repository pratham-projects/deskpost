---
type: TimeTrap
title: Time Traps
description: Things that burned time on Deskpost.
tags: [wisdom]
timestamp: 2026-10-27
---

# Time Traps

Carried over from bunfiiliate:

- `#[sqlx::test]` on one shared Postgres flakes at ~10 test threads: `.cargo/config.toml` pins 4, and each parallel worker uses its own scratch DB.
- BSD `sed -i` needs `''`; use python for edits.
- `pnpm`/`bun` interactive prompts hang scripts without a TTY: set `CI=true`.
- Workers keep committing after `worker_done`: check `git log staging..feat/x` before deleting a branch.
- Acknowledge a failed write only after rolling it back; reload config on the first change, rate-limit the rest.
- Rust 1.99 clippy flags `x % 2 != 0`; use `is_multiple_of`. Use a public `-> impl Future + Send` trait, not `async_trait`, when `dyn` is not needed.
- Crypto crates moved (aes-gcm 0.11, sha2/hkdf 0.13, rand 0.10): `from_slice` is deprecated (use `Key::from([u8;32])`, `Nonce::from([u8;12])`) and `rand::fill(&mut buf)` replaces `thread_rng().fill_bytes`. Check the registry source, not memory.
- proptest found a real bug in `build_references` (parent already in the chain was not last); always assert the invariant, not an example.
- mail-parser 1.x: `body_html(0)` fabricates HTML from a text-only mail (use `html_body()` + `is_content_type("text","html")`); `\u{80}..\u{9f}` count as control chars in header checks.
- IMAP `UID SEARCH n:*` always returns the highest message even if its UID < n: filter `> last_uid` client-side or the poller re-delivers the last mail forever.
- async-imap `Client::new` does not read the greeting (call `read_response()`); STARTTLS must be done on the raw socket before wrapping.


- Composite FK `ON DELETE SET NULL (col)` (PG15+) needs the referenced pair to be UNIQUE; use plain NO ACTION (not RESTRICT) on client_id so a project delete can cascade to clients and tickets in one statement.
- `#[sqlx::test]` binds `$n` types by first use; cast (`$4::text`) when a param is used in both COALESCE and comparisons.
- Refresh grace: when a parallel tab replays a just-rotated token, answer 401 but do NOT send a clearing `Set-Cookie`; it would delete the cookie the winning tab just received and log that tab out.
- `tower-http` CORS adds `Access-Control-Allow-Credentials` even for non-allowed origins (harmless without `Allow-Origin`); assert on `Allow-Origin`, not credentials. `Option<Layer>` is not a `Layer` for axum `Route`: apply conditional layers with `if`.
- Argon2 in debug builds takes seconds: tests build `Config` and override `argon2` with 8 KiB/t=1. `uuid` v4 needs the `v4` feature; `hmac::KeyInit` must be imported for `new_from_slice`.
- axum `Multipart` rejects a non-multipart content type with 400, not 415, and `Query<T>` failures are plain text: map both into `ApiError` yourself (`routes/tickets/mod.rs` `Params`, `create.rs`).
- macOS `sed -i` needs `-i ''`; prefer a small python script for multi-line edits.
- A route that writes two column groups (project + mail settings) must do it in one transaction, or a unique clash on the second half leaves the first applied; a `cat >>` heredoc run twice silently duplicated a whole test file (check `wc -l` after appending).
- macOS `sed -i` needs `''` and an `&&` chain stops at its failure: the following heredoc never ran.
- Limiter buckets are per IP and per rule: tests that sign in many times hit `Login` (10/min); give new endpoints their own `Rule`, and use `.peer(ip)` in tests that need a fresh bucket.
- `navigator.sendBeacon` can only POST: a "DELETE" via `_method` body registers a heartbeat instead. Use `fetch(..., {method:'DELETE', keepalive:true})` on unload.
- Turbopack does not dead-code-eliminate `require()`/`import()` behind `process.env.X === "1"`; the mock stayed in the bundle. Swap modules with `turbopack.resolveAlias` in `next.config.mjs` and grep `out/` for a seed string.

- Static export (`output: "export"`) cannot run under `next start`. Serve `out/` with Caddy; check the Docker target, not just the e2e Caddy path.
- **login to a chosen server proves nothing**: neither a login nor a code mailed through the tenant's own SMTP proves an address; only the operator (or DNS) can, so the operator approves.

- **pg_dump newer than the server**: a dump from `pg_dump` 18 fails to restore on PG 16 (`unrecognized configuration parameter "transaction_timeout"`). Use matching client tools; `scripts/backup.sh` refuses a mismatch.
- **systemd `EnvironmentFile` keeps inline `# comments` in the value** and lets the file override `Environment=`. Strip comments when installing `.env.example`.
- Outgoing mail belongs in the outbox, queued in the same transaction as the row it describes. Inline SMTP in a request holds it for the timeout, cannot retry, and (when sent before the commit) mails a message that was then rolled back. `claim_due` bumps `attempts`, so a crash loop needs its own dead-letter pass; size the lease to batch x per-job timeout.
- `tower::limit` needs the `limit` feature and `tower` as a normal dependency (it was dev-only); `tower_http::TimeoutLayer::new` is deprecated, use `with_status_code`.

- Parallel branches pick the next migration number independently: two `0004_*` files fail with a duplicate version at merge. Renumber the later one and bump the applied-count test.
- `sqlx::migrate!` embeds at compile time and cargo does not notice a newly added file, so tests fail with `VersionMissing(n)` until the db crate is rebuilt (`touch crates/db/src/lib.rs`).
- **Mass rename**: run `cargo fmt --all` before `make ci` (import order moves). `deploy-check` backup step needs `pg_dump` = server major (`PATH=/opt/homebrew/opt/postgresql@16/bin:$PATH`); site smoke needs `CHROME_PATH`. Env vars read in code must also be in compose and `.env.example`: `deploy-check` catches drift, so run it after any config change.
- **A new outbox kind changes every test that counts `mail_outbox`**: `new_ticket` (notice to members with `shouldNotify`) is queued on every API-created ticket because a project owner has the flag on, so `count == 1` asserts broke. Count by `kind`, clear unrelated rows in shared fixtures. Re-check old review findings against the tree before fixing: most R50 findings were already done.
- **Compose and systemd read `.env` inline comments as the value** when the value is empty: keep comments on their own lines in `.env.example`. After renaming the compose project, `down` the old project name (`-p <old>`) or it keeps running. zsh does not word-split `$VAR`: wrap compose in a function. `vite preview` needs the same `SITE_BASE` as the build.
