---
type: Architecture
title: Projects and team API (R21)
description: Routes for projects, members, users, clients, templates, email responses and project mail settings, with the secret and SSRF rules.
tags: [api, security, mail, tenancy]
timestamp: 2026-10-27
---

# Projects and team API (R21)

Contract: `docs/API.md` sections 5, 6, 8. Plumbing: [API crate](/architecture/api.md). All files live in `crates/api/src/routes/`; `routes::project_routes()` is the single line mounted by `build_app_with`.

| module | routes |
|--------|--------|
| `projects` | list/create/get/patch/delete, `regenerate-api-key`, `credentials` |
| `members` | list, search, add, patch, remove (`/projects/{id}/members...`) |
| `project_mail` | `MailFields` validation + sealing, `test-smtp`, `test-imap`, `password_aad`, `tls_for_port` |
| `mail_guard` | SSRF policy: `check_host` (syntax + names, no DNS), `check_resolved` (DNS), `is_blocked_ip` |
| `users` | me, profile, `POST /users/me/password`, admin CRUD with last-admin guard |
| `templates`, `email_responses`, `clients` | per-project resources (clients read only) |
| `qparams` | `Params` extractor (query errors become envelopes) with the `limit`/`offset` rules |

## Role matrix

Helpers: `project_role` (404 non-member), `project_admin` (403 agent). Project `owner` = `projects.owner_user_id`, always an admin member. Outsiders, including a global admin, always get 404, never 403. Resource ids (`/template-responses/{id}`, `/email-responses/{id}`, `/clients/{id}`) resolve through membership, so a foreign id answers exactly like a missing one. Templates: admin or `can_manage_templates`. Delete project: owner only (403 for other admins). Owner cannot be demoted or removed (409).

## Secrets

- Mail passwords are write-only: `MailFields` -> `CleanMail::into_update` seals with `SecretBox` and AAD `"{projectId}:{smtp|imap}_password"` (`project_mail::password_aad`). **The mail worker (R23) must open with the same AAD.** Responses expose only `hasSmtpPassword` / `hasImapPassword`.
- A changed host, port or username with a stored password needs the password re-sent (422), so an admin cannot redirect the stored secret to a new server. `""` clears.
- API key: `sdk_` + 256 bits, SHA-256 stored, prefix (12 chars) displayed, full key only in create/regenerate responses.
- Tests scan JSON bodies with `assert_no_secrets` (forbidden keys, `v1:` ciphertext, argon2 hashes, plaintext).

## SSRF

`SD_ALLOW_PRIVATE_MAIL_HOSTS` (`Config.allow_private_mail_hosts`) lifts the address rule only. Saving a host runs `check_host` (loopback/private/link-local/CGNAT/doc ranges, IPv4-mapped and NAT64 IPv6, `localhost`, `.local`/`.internal`/`.lan`, dotless names, numeric tails like `2130706433`). The probes also `check_resolved` (every DNS answer must be public) and are limited by `Rule::MailProbe` (5/min per IP). Probe answers are fixed strings, never server text. Residual risk: DNS can change between the check and the connect (sd-mail connects by name for TLS); the worker should re-run `check_resolved` before each connection.

## Decisions and hand-offs

- TLS is not stored (no schema change): ports 465/993 are implicit TLS, else STARTTLS.
- `PATCH /projects/{id}` applies project and mail columns in one transaction (`projects::update_with_mail`); `POST` creates the row, then the mail half, and deletes the new project if that half conflicts.
- `sd_db::users::{admin_update_guarded, delete_guarded}` keep one active admin (advisory lock, concurrency test).
- `DELETE /projects/{id}` removes attachment files itself through `FsStorage` on `STORAGE_DIR` (keys from `attachments::storage_keys_for_project`); failures are only logged. R23 may swap in the shared storage handle.
- Added beyond the old contract: `POST /users/me/password` (needs current password, revokes sessions, `Rule::PasswordChange`), read-only `/clients`.
- Collection routes answer both `/x` and `/x/` (the UI uses both).
