# Changelog

## [2026-10-27] R12 storage

- **Creation**: `sd-storage` (Storage trait, FsStorage, DownloadSigner, check_quota), [architecture page](/architecture/storage.md) with TS weaknesses. S3 driver deferred.
## [2026-10-27] R11 sd-core

- **Creation**: `crates/core` modules status, email_addr, threading, body, secret, api_key, filename, content_type, `constant_time_eq`; see [sd-core](/architecture/core.md). All `emailParser.test.ts` cases (and the extractor cases 1-20) rewritten as Rust tests plus proptest properties; 148 tests.
- **Decision**: TS bugs fixed instead of ported (RFC 2047 spoof, `From:` over-cut, bounce prefix match, key derivation); PDF kept download-only.
## [2026-10-27] R13 mail crate

- **Creation**: `sd-mail` (lettre sender, mail-parser inbound, async-imap `Mailbox`/`poll`, `testing` fakes), `docs/free-email.md`, [architecture/mail.md](/architecture/mail.md).
- **Lesson**: [mail-parser and IMAP gotchas](/wisdom/time-traps.md).
## [2026-10-27] R10 db + contract

- **Creation**: `docs/API.md`, `crates/db` (sd-db) with migration 0001, 13 query modules, 69 DB tests. See [DB crate and API contract](/architecture/db-contract.md).
- **Decision**: templates and clients are per project; API key hashed; `/clients` CRUD dropped; envelope `{success,...}` means R30 must change UI auth `status` checks.

## [2026-10-27] R00 scaffold

- **Creation**: Rust workspace (sd-core, sd-db, sd-mail, sd-storage, api), Makefile, CI, deny.toml, `docs/PLAN.md`, brain. Next.js demo moved to `apps/web`.
