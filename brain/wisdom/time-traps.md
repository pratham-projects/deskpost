---
type: TimeTrap
title: Time Traps
description: Things that burned time on SendDesk.
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
