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
- Crypto crates moved (aes-gcm 0.11, sha2/hkdf 0.13, rand 0.10): `from_slice` is deprecated (use `Key::from([u8;32])`, `Nonce::from([u8;12])`) and `rand::fill(&mut buf)` replaces `thread_rng().fill_bytes`. Check the registry source, not memory.
- proptest found a real bug in `build_references` (parent already in the chain was not last); always assert the invariant, not an example.
