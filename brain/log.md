# Changelog

## [2026-10-27] R11 sd-core

- **Creation**: `crates/core` modules status, email_addr, threading, body, secret, api_key, filename, content_type, `constant_time_eq`; see [sd-core](/architecture/core.md). All `emailParser.test.ts` cases (and the extractor cases 1-20) rewritten as Rust tests plus proptest properties; 148 tests.
- **Decision**: TS bugs fixed instead of ported (RFC 2047 spoof, `From:` over-cut, bounce prefix match, key derivation); PDF kept download-only.

## [2026-10-27] R00 scaffold

- **Creation**: Rust workspace (sd-core, sd-db, sd-mail, sd-storage, api), Makefile, CI, deny.toml, `docs/PLAN.md`, brain. Next.js demo moved to `apps/web`.
