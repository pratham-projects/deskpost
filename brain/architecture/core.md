---
type: Architecture
title: dp-core (pure logic crate)
description: Modules of crates/core, their contracts, and where they deliberately differ from the TypeScript product.
tags: [architecture, core, security, email]
timestamp: 2026-10-27
---

# dp-core

Pure logic, no IO, no async. Consumed by `dp-db`, `dp-mail`, `dp-storage`, `api`. Deps: aes-gcm, hkdf, sha2, subtle, base64, rand, regex (linear-time), idna; proptest (dev).

| module | contract |
|--------|----------|
| `status` | `TicketStatus` = pending, in_progress, on_hold, resolved, closed. Open states move freely; closed only reopens to pending; same-status is an error. `on_customer_reply` reopens resolved/closed/on_hold to pending. |
| `email_addr` | `extract_email_address`, `is_valid_email` (strict), `extract_and_validate_email`, `normalize_email` (lowercased lookup key), `response_email` (Reply-To, Sender, From, Return-Path), `is_bounce_message`. |
| `threading` | `ticket_subject` / `parse_ticket_number` (`[Ticket #N]`), `strip_reply_prefixes`, minted Message-IDs `<ticket-{uuid}[-reply-{uuid}]@domain>` and `ticket_id_from_message_id`, `parse_message_ids`, `build_references` (capped at 20, root kept). |
| `body` | `html_to_text` (bounded hand-rolled stripper), `clean_body` (reply/signature/forward stripping), `body_from_parts`, `is_attachment_only`. Input capped at 1 MiB. |
| `secret` | `SecretBox`: AES-256-GCM, random 96-bit nonce, HKDF-SHA256 key from a 32+ byte secret, `v1:<b64url(nonce‖ct‖tag)>`, caller-supplied AAD (bind project id + field). |
| `api_key` | `generate_api_key` -> (`sdk_` + 256-bit random, SHA-256 hex); `verify_api_key` constant time. |
| root | `constant_time_eq` (compares SHA-256 digests, hides length). |
| `filename` | `sanitize_filename`: last path component, no control/bidi/separator chars, reserved device names prefixed, no dotfiles, <=120 bytes, idempotent. |
| `content_type` | `classify` / `served_content_type`: inline only for png/jpeg/gif/webp/avif/bmp/text plain+csv; SVG, HTML, XML, PDF and anything malformed are download-only as `application/octet-stream`. |

## Fixed on purpose (not ported from TS)

- RFC 2047 words are stripped, never decoded, before address extraction (decoding allowed display-name spoofing of the sender).
- Bare `From:` line is a quote header only when `Sent:`/`Date:` follows (TS cut messages at any `From:`).
- Leading-header stripping needs 2+ header lines; `Sent from my ...` signature only for short device phrases.
- Bounce check matches the exact local part (TS prefix regex flagged `noreplyfan@`).
- Encryption: TS padded the secret with `0` into the key, used no KDF, no version tag, no AAD.
- Address parser rejects backslashes outside quoted-strings and unbalanced quotes/comments (TS accepted `\"John\" <a@b>`).
- Subject helper strips control characters from the project name (header injection).
- HTML `<blockquote>` becomes `>` lines so HTML-only replies are stripped.

## Notes

- MIME parsing (headers, multipart, attachments) belongs to `dp-mail`; `dp-core` takes already-extracted text/html.
- `SecretBox` does not zeroize key material (no `zeroize` crate); the process holds the key for its lifetime anyway.
- PDF is download-only by choice; revisit only with a sandboxed viewer origin.
