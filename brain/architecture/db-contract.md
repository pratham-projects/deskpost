---
type: Architecture
title: DB crate and API contract
description: dp-db schema, tenant-scoping rules, query module map and the docs/API.md contract (R10).
tags: [db, api, tenancy]
timestamp: 2026-10-27
---

# DB crate and API contract

- HTTP contract: `docs/API.md` (envelope `{success,data|error}`, auth, every endpoint, 20 listed deviations from the TS product).
- Schema: `crates/db/migrations/0001_init.sql`, Postgres 16, UUID v7 ids from Rust, timestamptz, CHECKs for every status.
- Tenant = project. Child tables carry `project_id` and use composite FKs `(project_id, x_id)`, so cross-tenant rows are impossible even with raw SQL. Assignee FK `(project_id, assigned_to_user_id)` -> members, `SET NULL` on member removal.
- Ticket numbers: `projects.next_ticket_number`, incremented under the project row lock inside the create transaction (50 parallel creates give 1..50).
- Email dedup: unique `(project_id, email_message_id)`. Inbound routing: unique `lower(smtp_email)`.
- Refresh: `refresh_families` + `refresh_tokens` (`rotated_at`, grace window, reuse revokes family).
- Secrets: only `*_encrypted` columns; `Project` struct exposes `has_*_password`, `MailSettings` Debug hides ciphertext. API key stored as hash + prefix.
- Query API: functions take `actor` (user id) and check membership/role in SQL; `None`/`false` = not found or not allowed. `members::role_of` separates 403 from 404. Functions without actor are system side (widget API key via `tickets::Access`, inbound mail, workers).
- Modules: users, sessions, projects, members, clients, tickets, messages, attachments, templates, email_responses, email_logs, unread, viewers.
- Tests: `crates/db/tests/*.rs` with `#[sqlx::test(migrations = "./migrations")]`.
