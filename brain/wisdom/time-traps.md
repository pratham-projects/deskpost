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

- Composite FK `ON DELETE SET NULL (col)` (PG15+) needs the referenced pair to be UNIQUE; use plain NO ACTION (not RESTRICT) on client_id so a project delete can cascade to clients and tickets in one statement.
- `#[sqlx::test]` binds `$n` types by first use; cast (`$4::text`) when a param is used in both COALESCE and comparisons.
