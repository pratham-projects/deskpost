# Changelog

All notable changes to Deskpost are documented here. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning: [SemVer](https://semver.org/).

## [Unreleased]

## [0.1.0-rc.2] - 2026-11-19

First pre-release (`v0.1.0-rc.2`; rc.1 never built, its tag did not match the crate version), not yet recommended for production. The API may still change before `v0.1.0`.

### Added
- Multi-tenant support-ticket inbox: Rust API (Axum, SQLx, Postgres 16) and Next.js web UI.
- Ticket lifecycle: create, assign, reply, internal notes, status and priority changes.
- Email in and out via SMTP, with per-tenant mail configuration and encrypted secrets (`DP_SECRET_KEY`).
- Attachment storage on local disk.
- Webhooks, rate limiting, request body caps, JWT auth with bootstrap admin.
- `deskpost` binary (`deskpost serve`, migrations), Docker images (`deskpost-api`, `deskpost-web`), Docker Compose stack, Caddy and systemd deploy files.
- Release tooling: cargo-dist builds for macOS and Linux (aarch64, x86_64), shell installer, checksums.
- CI: fmt, clippy, tests, cargo-deny, web build, Docker build check, deploy-file check.
- AGPL-3.0-only license, project site and docs.

[Unreleased]: https://github.com/pratham-projects/deskpost/compare/v0.1.0-rc.2...HEAD
[0.1.0-rc.2]: https://github.com/pratham-projects/deskpost/releases/tag/v0.1.0-rc.2
