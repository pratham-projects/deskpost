# Changelog

## [2026-10-27] R10 db + contract

- **Creation**: `docs/API.md`, `crates/db` (sd-db) with migration 0001, 13 query modules, 69 DB tests. See [DB crate and API contract](/architecture/db-contract.md).
- **Decision**: templates and clients are per project; API key hashed; `/clients` CRUD dropped; envelope `{success,...}` means R30 must change UI auth `status` checks.

## [2026-10-27] R00 scaffold

- **Creation**: Rust workspace (sd-core, sd-db, sd-mail, sd-storage, api), Makefile, CI, deny.toml, `docs/PLAN.md`, brain. Next.js demo moved to `apps/web`.
