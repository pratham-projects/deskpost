---
title: HTTP API
group: Features
order: 7
description: The /api/v3 contract in brief: envelope, auth, routes and errors.
---

# HTTP API, v3

Everything is under `/api/v3` (`API_VERSION`), except `GET /health` and `GET /health/db`. The full contract, including every field and deviation from the product it replaced, is `docs/API.md` in the repository.

JSON in and out, camelCase fields, UUID ids, RFC 3339 UTC timestamps. Bodies are capped at 64 KiB by default (`DP_BODY_LIMIT_BYTES`); upload and webhook routes raise their own cap. Unknown JSON fields are ignored.

## Envelope

```json
{ "success": true, "data": {}, "pagination": { "total": 0, "limit": 50, "offset": 0, "hasMore": false } }
{ "success": false, "error": { "code": "NOT_FOUND", "message": "Ticket not found." } }
```

## Auth

Sign in with `POST /auth/signin`. You get a JWT access token (HS256, 5 minutes) for `Authorization: Bearer`, and an opaque refresh token in an `HttpOnly` cookie scoped to `/api/v3/auth`. `POST /auth/refresh` rotates it. `POST /auth/signout` revokes the family and `POST /auth/signout-all` revokes every family of the caller. `GET /auth/config` reports `signupEnabled`.

## Routes

| Area | Routes |
| --- | --- |
| Tickets | `GET /tickets`, `/tickets/search`, `/tickets/counts`, `/tickets/:id`; `POST /tickets/admin`; `PATCH` and `DELETE /tickets/:id` |
| Messages | `GET` and `POST /tickets/:id/messages`; `isInternal` makes a note, `sendEmail: false` skips mail |
| Attachments | `PUT /attachments/:ticketId`, `POST /attachments/:ticketId/multiple`, `GET /attachments/file/:id` |
| Projects | `/projects`, members, `regenerate-api-key`, `test-smtp`, `test-imap`, `mailbox/approve` |
| Templates | `/template-responses` (canned replies), `/email-responses` (auto-replies) |
| Unread, presence | `/unread`, `/viewers/:ticketId` |
| Public | `POST /tickets` with `X-Project-Key`, `POST /incoming-email` with a signature |

Lists default to 50 and cap at 200. Tickets order by `updatedAt desc`, or unread first with `unreadFirst=true`. `GET /tickets` also returns per-status `stats`.

## Statuses

`pending`, `in_progress`, `on_hold`, `resolved`, `closed`. Open statuses move to any other status. A closed ticket can only be reopened to `pending`. Priorities are `low`, `normal`, `high`, `urgent`.

## Errors

| HTTP | Code | Meaning |
| --- | --- | --- |
| 401 | `TOKEN_MISSING`, `TOKEN_EXPIRED`, `TOKEN_INVALID` | The UI refreshes on these. |
| 401 | `INVALID_CREDENTIALS` | Same message for an unknown email and a wrong password. |
| 403 | `FORBIDDEN` | A member whose role is too low. |
| 404 | `NOT_FOUND` | Missing, or not visible to the caller. |
| 409 | `CONFLICT` | Unique violation. |
| 413, 415 | `PAYLOAD_TOO_LARGE`, `UNSUPPORTED_MEDIA_TYPE` | Over a cap, or a type that is not allowed. |
| 422 | `VALIDATION_ERROR` | `details` maps field to message. |
| 429 | `RATE_LIMITED` | `Retry-After` is set. |

## Health

`GET /health` is liveness with no database access. `GET /health/db` runs `SELECT 1` with a 2 second timeout and answers `503` when Postgres is down. Alert on that one.
