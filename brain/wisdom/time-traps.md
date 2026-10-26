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
- mail-parser 1.x: `body_html(0)` fabricates HTML from a text-only mail (use `html_body()` + `is_content_type("text","html")`); `\u{80}..\u{9f}` count as control chars in header checks.
- IMAP `UID SEARCH n:*` always returns the highest message even if its UID < n: filter `> last_uid` client-side or the poller re-delivers the last mail forever.
- async-imap `Client::new` does not read the greeting (call `read_response()`); STARTTLS must be done on the raw socket before wrapping.

