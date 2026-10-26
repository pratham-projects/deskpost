---
type: Architecture
title: Storage crate (sd-storage)
description: Attachment store trait, filesystem driver, signed download tokens, quota helper, and TS-product weaknesses.
tags: [architecture, storage, security]
timestamp: 2026-10-27
---

# sd-storage

`Storage` trait (`put` stream -> `StoredObject`, `get`, `delete`, `exists`; native async-fn-in-trait, no `dyn`).
Only driver: `FsStorage`. S3 driver is **future work**, deliberately out of scope.

## Filesystem driver

- Keys are server generated: `ab/cd/abcd<28 hex>` (uuid v4 simple, two-level fan-out). `parse_key` accepts only that exact shape, so `..`, absolute, NUL, unicode lookalikes, uppercase, wrong fan-out are `InvalidKey` before any IO.
- Every path component under the root is `symlink_metadata`-checked; symlinks (fan-out dirs, `.tmp`, object file) give `Unsafe`. The configured root itself is canonicalised once. Residual TOCTOU needs write access to the root, which already means game over.
- Write: stream into `<root>/.tmp/<id>.data`, size cap enforced per chunk (abort -> `TooLarge`), sha256 computed in-stream, fsync, then sidecar `.json` renamed in, then data renamed in. A `TempGuard` removes temp files on error **and** on future cancellation.
- Original filename and content type are metadata only (`sanitize_filename`, `sanitize_content_type`); never part of a path.
- `delete` is idempotent. `exists`/`get` need both data and sidecar.

## Tokens and quota

- `DownloadSigner`: `<expires_at>.<i|a>.<hex hmac-sha256>` over a length-prefixed `(key, expires_at, disposition)`. Secret >= 32 bytes. Verify is constant time (`Mac::verify_slice`), signature before expiry, 30 s leeway (`LEEWAY_SECS`), canonical-integer expiry only. Caller passes `now`.
- `check_quota(used, incoming, limit)`: caller supplies the project's summed size; overflow counts as exceeded.

## Limits to wire in the API (R23)

TS defaults: 10 MiB per attachment (`EMAIL_MAX_ATTACHMENT_SIZE`), 20 MiB per email (`EMAIL_MAX_SIZE`). The driver takes its cap at construction; per-email total and per-project quota are API concerns.

## Weaknesses of the TS product (`fileStorage.ts`, `routes/attachments.ts`), not ported

1. Whole upload buffered in memory (`arrayBuffer`) before the size check; no streaming cap.
2. S3 key is `tickets/<id>/<attachmentId>/<client filename>`: user input in the key; filename only "safe" by a `|| file-<ts>` fallback.
3. Stored `Content-Disposition: inline` with the raw filename in quotes (header injection / XSS via inline HTML/SVG); downloads also inline with the client-claimed MIME type.
4. Download route `/file/:attachmentId` and ticket listing do no tenant check (any authed user, ID guess).
5. `getFile` parses arbitrary stored URLs, supports cross-bucket access and fallback buckets: SSRF-ish key confusion.
6. Attachment row inserted before upload and deleted on failure; no atomicity, no cleanup of orphaned objects; no sha256; no per-project quota.
7. Multi-upload validates only the sum after reading everything; no dedupe of failures mid-batch (earlier files stay uploaded).
8. Public S3 URL stored and returned in API responses.
