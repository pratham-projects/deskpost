# SendDesk HTTP API contract (v3)

Source of truth for R20-R23 (server) and R30 (UI wiring). Derived from `apps/web/mock/*`,
`apps/web/lib/api/*.ts`, `apps/web/lib/types.ts` (what the UI needs) and cross-checked with
`~/code/sendesk-jobs/backend/src` (what the TS product does). Where the two disagree or the TS product
is wrong, this document decides; every such case is listed in [Deviations](#deviations-from-the-ts-product).

## 1. Conventions

- Base path `/api/v3` (`API_VERSION=v3`; the UI reads `NEXT_PUBLIC_API_VERSION`). Only `/health` is outside it.
- JSON in/out (`Content-Type: application/json`) unless an endpoint says multipart or binary.
- IDs are UUIDs (strings). Timestamps are RFC 3339 UTC strings (`2026-10-27T10:00:00Z`). Field names are camelCase.
- Request bodies are capped (default 64 KiB, `SD_BODY_LIMIT_BYTES`; upload and webhook routes raise their own cap, section 9). Over the cap: `413 PAYLOAD_TOO_LARGE`.
  Unknown JSON fields are ignored; missing/invalid required fields give `422 VALIDATION_ERROR`.

### Envelope

```jsonc
// success
{ "success": true, "data": <T>, "pagination": {...}?, "stats": {...}? }
// error
{ "success": false, "error": { "code": "NOT_FOUND", "message": "Ticket not found.", "details": {...}? } }
```

`pagination` = `{ "total": n, "limit": n, "offset": n, "hasMore": bool }`, present on paginated lists.
`client.ts` already reads `error.code` / `error.message`, so no client change is needed for errors.

### Error codes

| HTTP | code | meaning |
|------|------|---------|
| 400 | `BAD_REQUEST` | malformed request (bad UUID, bad multipart, empty upload) |
| 401 | `TOKEN_MISSING` | no `Authorization: Bearer` header |
| 401 | `TOKEN_EXPIRED` | access token expired (client must refresh) |
| 401 | `TOKEN_INVALID` | signature/format wrong, or user inactive |
| 401 | `INVALID_CREDENTIALS` | sign-in failed (same message for unknown email and wrong password) |
| 403 | `FORBIDDEN` | authenticated, member of the project, but role too low |
| 405 | `METHOD_NOT_ALLOWED` | known path, wrong method (unknown paths are `404 NOT_FOUND`) |
| 404 | `NOT_FOUND` | resource missing **or not visible to the caller** (tenant isolation never leaks existence) |
| 409 | `CONFLICT` | unique violation (email, slug, domain, status already has an email response, ...) |
| 413 | `PAYLOAD_TOO_LARGE` | body/file over cap; `details` lists offending files |
| 415 | `UNSUPPORTED_MEDIA_TYPE` | upload content type not allowed |
| 422 | `VALIDATION_ERROR` | field validation; `details` = `{ field: message }` |
| 429 | `RATE_LIMITED` | `Retry-After` header set |
| 500 | `INTERNAL` | generic message, details only in server logs |
| 502 | `MAIL_DELIVERY_FAILED` | outbound SMTP failed; nothing was saved (see 6.7) |

401 codes `TOKEN_MISSING`, `TOKEN_EXPIRED`, `TOKEN_INVALID` are exactly what `client.ts` matches to trigger refresh.

### Authentication and roles

- Access token: JWT HS256, 5 min (`expiresIn` seconds in the response; UI subtracts 30 s). Sent as `Authorization: Bearer <jwt>`.
- Refresh token: opaque random, stored hashed server-side, rotated on every use. Delivered as cookie
  `refresh_token` (`HttpOnly; Path=/api/v3/auth; SameSite=Strict` in production / `Lax` in dev; `Secure` in production;
  `Max-Age` 7 days, sliding). Absolute family lifetime 90 days.
  Accepted from the cookie, else from the `X-Refresh-Token` header (cross-origin dev setups).
  The token is also returned in the JSON body (`refreshToken`) **only when `SD_ENV != production`**, as in the TS product;
  the UI stores it in localStorage for those setups.
- Reuse of an already-rotated refresh token revokes the whole family (`revoked_reason = reuse_detected`), except a replay within
  10 s of the rotation, which is answered with `401 TOKEN_EXPIRED` but does **not** revoke (parallel tabs).
- CORS: credentials allowed, origin allow-list from config; allowed headers `Content-Type, Authorization, X-Refresh-Token, X-Project-Key, X-Filename, X-Message, X-Send-Email, X-Webhook-Signature, X-Webhook-Timestamp`.
- Global user role: `admin` (can manage user accounts) or `agent`. Everything about project data is decided by **project
  membership** (`project_members.role`: `admin` | `agent`), never by the global role. The project owner is always a member with role `admin`.
- Access levels used below: **public**, **user** (any signed-in user), **member** (member of the ticket's/resource's project),
  **project-admin** (member with role `admin`), **global-admin**.
  A non-member always gets `404 NOT_FOUND` (not 403) for project-scoped resources.
- Rate limits (in-process sliding window, per client IP): signin/signup/refresh, public ticket create, webhook.

## 2. Shared shapes

```ts
type TicketStatus = "pending" | "in_progress" | "on_hold" | "resolved" | "closed"
type Priority = "low" | "normal" | "high" | "urgent"

interface User {            // public user
  id: string; email: string; name: string; role: "admin" | "agent"
  status: "active" | "inactive" | "suspended"
  phone: string | null; timezone: string; avatarUrl: string | null
  lastLoginAt: string | null; createdAt: string; updatedAt: string
}
interface Project {
  id: string; name: string; slug: string; domain: string | null; description: string | null
  status: "active" | "inactive" | "suspended"; isArchived: boolean
  apiKeyPrefix: string                 // e.g. "pk_1a2b3c4d"; full key only in create/regenerate responses (`apiKey`)
  ownerUserId: string
  replyToEmail: string | null
  smtpEmail: string | null; smtpUsername: string | null; smtpHost: string; smtpPort: number; hasSmtpPassword: boolean
  imapHost: string | null; imapPort: number | null; imapUsername: string | null; imapFolder: string; hasImapPassword: boolean
  createdAt: string; updatedAt: string
}
interface ProjectMember {
  id: string; projectId: string; userId: string; role: "admin" | "agent"
  shouldNotify: boolean; canViewAllTickets: boolean; canManageTemplates: boolean
  createdAt: string; updatedAt: string
  user: { id: string; email: string; name: string }
}
interface Ticket {
  id: string; ticketNumber: number           // per-project, 1..n
  projectId: string; clientId: string
  subject: string; description: string | null
  status: TicketStatus; priority: Priority
  assignedToUserId: string | null
  createdAt: string; updatedAt: string; closedAt: string | null
  client: { id: string; email: string; name: string | null }
  project: { id: string; name: string }
  assignedTo: { id: string; email: string; name: string } | null
}
interface Message {
  id: string; ticketId: string
  senderType: "agent" | "client"; senderUserId: string | null
  senderEmail: string | null; senderName: string | null
  body: string; htmlBody: string | null
  messageType: "reply" | "note" | "system"
  isInternal: boolean; isPublished: boolean
  attachmentIds: { id: string; url: string }[]   // url = `/api/v3/attachments/file/{id}` (relative, auth required)
  createdAt: string; updatedAt: string
  sender: { id: string; email: string; name: string } | null
}
interface Attachment { id: string; ticketId: string; messageId: string | null; filename: string; mimeType: string; sizeBytes: number; createdAt: string }
interface TemplateResponse { id: string; projectId: string; title: string; body: string; createdAt: string; updatedAt: string }
interface EmailResponse {   // auto-reply sent to the client on ticket events
  id: string; projectId: string; status: "created" | "resolved"
  subject: string; body: string; isEnabled: boolean; createdAt: string; updatedAt: string
}
interface TicketStats { pending: number; in_progress: number; on_hold: number; resolved: number; closed: number; all: number }
interface UnreadItem { id: string; ticketId: string; projectId: string; createdAt: string }
```

## 3. Health

| Method / path | Auth | Response |
|---|---|---|
| `GET /health` | public | liveness: `200 { success, data: { status: "ok" } }`, no database access |
| `GET /health/db` | public | readiness: `200 { success, data: { status: "ok", database: "up" } }`; `503 { success: false, data: { status: "error", database: "down" } }` when Postgres does not answer within 2 s. No table names, no versions. |

## 4. Auth (`/api/v3/auth`)

| Endpoint | Auth | Request | Response | Errors |
|---|---|---|---|---|
| `POST /auth/signup` | public, rate limited | `{ email, password (8..128), name (1..255) }` | `201 data: { id, email, name, role }` | 409 `CONFLICT` email taken (case-insensitive), 422, 429 |
| `POST /auth/signin` | public, rate limited | `{ email, password }` | `200 data: { user: User, accessToken, expiresIn, refreshToken? }` + `Set-Cookie: refresh_token` | 401 `INVALID_CREDENTIALS` (also for inactive/suspended accounts, same message), 422, 429 |
| `POST /auth/refresh` | refresh token (cookie or `X-Refresh-Token`) | none | same shape as signin, new cookie | 401 `TOKEN_MISSING` (none supplied), 401 `TOKEN_EXPIRED` (invalid/expired/revoked/reused; cookie cleared) |
| `POST /auth/signout` | refresh token optional | none | `200 data: { message }`; revokes the token's family; cookie cleared. Always 200. | |
| `POST /auth/signout-all` | user | none | `200 data: { message }`; revokes every family of the caller | 401 |
| `GET /auth/me` | user | none | `200 data: { user: User }` | 401 |
| `GET /auth/config` | public | none | `200 data: { signupEnabled: bool }` (`SD_ALLOW_SIGNUP`, default true; when false, `POST /auth/signup` is `403 FORBIDDEN` and the first admin comes from `api bootstrap`) | |

Bootstrap: the first user ever registered gets role `admin`; every later signup is `agent`.
Passwords are Argon2id; the account is looked up by lower-cased email.
Sign-in with a user whose `status != active` fails with the generic credentials error.

## 5. Users (`/api/v3/users`)

| Endpoint | Auth | Request | Response | Errors |
|---|---|---|---|---|
| `GET /users/me` | user | | `data: User` | 401 |
| `PATCH /users/me` | user | `{ name?, phone?, timezone? }` (all optional; `phone` max 20, `name` 1..255) | `data: User` | 422 |
| `GET /users` | user | query `role?`, `status?`, `limit?`, `offset?` | `data: User[]` + `pagination`. Global-admin: all users. Others: only users sharing at least one project with the caller. | |
| `POST /users` | global-admin | `{ email, password, name, role?: "admin"\|"agent" }` | `201 data: User` | 403, 409, 422 |
| `GET /users/:id` | user (self, global-admin, or shares a project) | | `data: User` | 404 |
| `PATCH /users/:id` | global-admin | `{ name?, role?, status?, phone?, timezone? }` | `data: User` | 403, 404, 422. Cannot demote/suspend the last active admin (409). |
| `DELETE /users/:id` | global-admin | | `data: { success: true, id }` | 403, 404, 409 when the user still owns a project or is the last active admin |
| `POST /users/me/password` | user, rate limited (6/min per IP) | `{ currentPassword, newPassword (8..128) }` | `data: { message }`; every refresh family of the caller is revoked | 422 (`currentPassword` wrong), 429 |

Deleting a user cascades their memberships and refresh families; their tickets/messages are kept (assignee/sender set to null).

## 6. Projects (`/api/v3/projects`)

A project is the tenant. `GET /projects` returns only projects the caller is a member of.

| Endpoint | Auth | Request | Response | Errors |
|---|---|---|---|---|
| `GET /projects` | user | query `status?`, `limit?`, `offset?` | `data: Project[]` + `pagination` | |
| `POST /projects` | user | `{ name (1..255), slug (3..255, [a-z0-9-]), domain?, description?, replyToEmail?, smtpEmail?, smtpUsername?, smtpPassword?, smtpHost?, smtpPort?, imapHost?, imapPort?, imapUsername?, imapPassword?, imapFolder? }` | `201 data: Project & { apiKey }`; caller becomes owner + `admin` member; ticket counter starts at 1 | 409 slug/domain/`smtpEmail` taken (the mailbox address routes inbound mail, so it must identify one project), 422 |
| `GET /projects/:id` | member | | `data: Project` | 404 |
| `PATCH /projects/:id` | project-admin | any of the create fields plus `status?`, `isArchived?`. Passwords are write-only; omitted = unchanged, `""` = clear. | `data: Project` | 403, 404, 409, 422 |
| `DELETE /projects/:id` | owner | | `data: { success: true, id }`; cascades tickets, messages, attachments (files removed by the API), members, templates, email responses | 403, 404 |
| `POST /projects/:id/regenerate-api-key` | project-admin | | `data: { id, apiKey, updatedAt }`; old key stops working immediately | 403, 404 |
| `GET /projects/:id/credentials` | project-admin | | `data: { email, username, smtpHost, smtpPort, hasSmtpPassword, imapHost, imapPort, imapUsername, imapFolder, hasImapPassword }` - **never the passwords** | 403, 404 |
| `GET /projects/:id/members` | member | | `data: ProjectMember[]` | 404 |
| `GET /projects/:id/members/search` | member | query `q` (may be empty), `limit?`, `offset?`; matches member name/email, case-insensitive | `data: ProjectMember[]` | 404 |
| `POST /projects/:id/members` | project-admin | `{ userId, role?: "admin"\|"agent" (default agent), shouldNotify? }` | `201 data: ProjectMember` | 403, 404 (unknown user), 409 already a member |
| `PATCH /projects/:id/members/:userId` | project-admin | `{ role?, shouldNotify?, canViewAllTickets?, canManageTemplates? }` | `data: ProjectMember` | 403, 404; 409 when it would demote the project owner (a project always keeps an admin) |
| `DELETE /projects/:id/members/:userId` | project-admin, or the member removing themself | | `data: { success: true }` | 404; 409 for the owner. Tickets assigned to the removed member become unassigned. |

| `POST /projects/:id/test-smtp` | project-admin, rate limited (5/min per IP, shared with IMAP) | `{ host?, port?, tls?: "starttls"\|"implicit"\|"none", username?, password? }`; omitted fields come from the stored settings | `data: { ok, message }` (fixed messages, no server text) | 403, 404, 422, 429 |
| `POST /projects/:id/test-imap` | same | same (`port` defaults to 993) | same | same |

Notes:
- The stored password is only used for a probe when host, port and username equal the stored ones; any other target needs `password` in the request (422 `password`). Resolved addresses are checked too (a name that resolves to a private address is refused). `tls: "none"` needs `SD_ALLOW_PRIVATE_MAIL_HOSTS=1`.
- TLS is not stored: port 465/993 means implicit TLS, everything else STARTTLS.
- Changing `smtpHost`/`smtpPort`/`smtpUsername` (or the IMAP equivalents) while a password is stored requires the password again in the same PATCH (`""` clears it), otherwise 422. PATCH applies project and mail fields in one transaction.
- `DELETE /projects/:id` is owner only (403 for other admins); the owner cannot be demoted or removed (409).
- SMTP/IMAP hosts are validated: loopback/private targets are rejected unless `SD_ALLOW_PRIVATE_MAIL_HOSTS=1` (`422`).
- `smtpPort` defaults to 587, `smtpHost` has **no** provider default (TS defaulted to `smtp.gmail.com`); outbound mail needs a configured host or the reply endpoint returns 422 `SMTP not configured`.
- `POST /projects` auto-created `canViewAllTickets=true` for the owner.

## 7. Tickets (`/api/v3/tickets`)

Route order matters (literal paths before `/:id`): `POST /admin`, `GET /counts`, `GET /search` come before `/:id`.

### 7.1 Create

| Endpoint | Auth | Request | Response |
|---|---|---|---|
| `POST /tickets/admin` | member of `projectId` | multipart/form-data | `201 data: Ticket` |
| `POST /tickets` | public (widget), rate limited; header `X-Project-Key: <project api key>` required | multipart/form-data | `201 data: Ticket` |

Multipart fields (both): `projectId` (uuid, required), `clientEmail` (valid email, required), `clientName?`, `subject` (5..255), `description` (1..20000; TS required it on the wire, the mock allowed empty - required here), `priority?` (default `normal`), `sendNotification?` ("true"/"false"; default true), `files` (0..n, caps in section 9).

Behaviour:
1. Client = get-or-create by lower-cased email **within the project**; `clientName` defaults to the local part of the email.
2. `ticketNumber` = project's next counter value (atomic, gap-free per project).
3. Status `pending`; first message `senderType: "client"` (public) or `"client"` with `senderUserId = creator` for `/admin` (a ticket created on behalf of a client is still a client message - the mock marked it `agent`; TS marked it client). For `/admin` the creator is recorded in `createdByUserId`, ticket stays unassigned (the mock auto-assigned; TS did not).
4. Unread row inserted for the ticket.
5. If the project has an enabled `created` email response and `sendNotification`, the confirmation mail is sent **after commit**; a mail failure never fails ticket creation (it is logged to `email_logs` with status `failed`).
6. Team members with `shouldNotify` get a "new ticket" mail (best effort).

Errors: 403/404 (admin route, not a member), 401 `TOKEN_INVALID` on the public route for a bad/missing project key, 413, 415, 422, 429.

### 7.2 Read

| Endpoint | Auth | Request | Response |
|---|---|---|---|
| `GET /tickets` | user | query `projectId?`, `status?`, `assignedToUserId?`, `priority?`, `limit?` (default 50, max 200), `offset?`, `unreadFirst?` ("true") | `data: Ticket[]`, `pagination`, `stats: TicketStats` |
| `GET /tickets/search` | user | query `q` (min 2 chars), `projectId?`, `limit?`, `offset?` | `data: Ticket[]` + `pagination`; case-insensitive literal match (`%`/`_` are not wildcards) on subject, description and message bodies; ordered by `updatedAt desc` (internal notes are visible to every member, so they are searched too) |
| `GET /tickets/counts` | user | | `data: { global: TicketStats, projects: { [projectId]: TicketStats } }` over the caller's projects |
| `GET /tickets/:id` | member | | `data: Ticket` |

- Only tickets of projects the caller is a member of are ever returned. A `projectId` the caller is not a member of yields an empty list (list/search) - not an error - and 404 for `/:id`.
- Members with `canViewAllTickets = false` only see tickets assigned to them or created by them.
- Default order `updatedAt desc, id desc`; with `unreadFirst=true`, tickets with an unread row sort first. `stats` is computed for the same project filter (all projects of the caller when `projectId` is absent), ignoring `status`/`priority`/`assignedToUserId`.

### 7.3 Update / delete

| Endpoint | Auth | Request | Response |
|---|---|---|---|
| `PATCH /tickets/:id` | member | `{ subject?, description?, status?, priority?, assignedToUserId? (uuid\|null) }` | `data: Ticket` |
| `DELETE /tickets/:id` | project-admin | | `data: { success: true, id }` (hard delete, cascades messages/attachments/unread/viewers) |

- `assignedToUserId` must be a member of the ticket's project (422 otherwise); `null` unassigns.
- `status` change rules (state machine in `sd-core`; the query that persists it is `sd_db::tickets::update`): moves follow the state machine (an open status may go to any other; a `closed` ticket can only be reopened to `pending`, else `409 CONFLICT`; re-sending the current status is a no-op); entering `resolved` or `closed` sets `closedAt = now()` (if not already set); leaving them clears `closedAt` (TS never cleared it on reopen). Entering `resolved` sends the `resolved` email response (if enabled) after commit.
- Every successful update bumps `updatedAt`.

### 7.4 Messages

| Endpoint | Auth | Request | Response |
|---|---|---|---|
| `GET /tickets/:id/messages` | member | query `limit?` (default 50, max 200), `offset?` | `data: Message[]` newest first + `pagination` |
| `POST /tickets/:id/messages` | member | `{ body (1..50000), isInternal?: bool, sendEmail?: bool, attachmentIds?: uuid[] }` | `201 data: Message` |

- Created messages are always `senderType: "agent"` with `senderUserId` = caller. (TS let an agent post as `client` and could reopen tickets with it; removed. Client messages arrive only via public create, IMAP and the webhook.)
- `isInternal: true` => stored as an internal note (`messageType: "note"`), never emailed.
- Otherwise, unless `sendEmail === false`, the reply is handed to the outbound mailer (`OutboundMailer`, R23) **after the message commits**. A mail failure is logged and recorded in `email_logs` (`failed`); the endpoint still answers `201` (R22 decision, supersedes the earlier "mail first, 502" idea; R23 may revisit).
- `attachmentIds` must belong to the same ticket and be unlinked; they are linked to the message (422 otherwise).
- Posting bumps ticket `updatedAt`; a first agent reply sets `firstResponseAt`.
- Inbound client messages (IMAP/webhook, not an HTTP route here) reopen `resolved`/`closed` tickets to `in_progress`, add an unread row, and de-duplicate on the email `Message-ID`.

## 8. Templates, email responses

### Templates (canned replies) - `/api/v3/template-responses`

Project-scoped (TS stored them globally, visible to every user; the UI must now pass `projectId`).

| Endpoint | Auth | Request | Response |
|---|---|---|---|
| `GET /template-responses/` | member | query `projectId` (required), `limit?` (50), `offset?` | `data: TemplateResponse[]` + `pagination`, ordered by title |
| `GET /template-responses/search` | member | query `projectId` (required), `q`, `limit?`, `offset?`; case-insensitive on title and body | same |
| `POST /template-responses/` | member with `canManageTemplates` or project-admin | `{ projectId, title (1..255), body }` | `201 data: TemplateResponse` |
| `PATCH /template-responses/:id` | same | `{ title?, body? }` | `data: TemplateResponse` |
| `DELETE /template-responses/:id` | same | | `data: { success: true }` |

`GET /template-responses/:id` (TS only) is dropped - the UI never reads a single template.

### Email responses (auto-replies per project and ticket event) - `/api/v3/email-responses`

| Endpoint | Auth | Request | Response | Errors |
|---|---|---|---|---|
| `GET /email-responses/` | member | query `projectId` (required), `status?` | `data: EmailResponse[]` | 404 |
| `POST /email-responses` | project-admin | `{ projectId, status: "created"\|"resolved", subject? (<=255), body (1..20000), isEnabled? (default true) }` | `201 data: EmailResponse` | 409 an entry for that status already exists (unique per project+status), 422 |
| `PATCH /email-responses/:id` | project-admin | `{ subject?: string\|null, body?, isEnabled? }` | `data: EmailResponse` | 404 |
| `DELETE /email-responses/:id` | project-admin | | `data: { success: true }` | 404 |

Statuses are restricted to `created` and `resolved` (the only two the UI uses and the only two the TS product ever sends mail for; `pending`/`in_progress` rows existed in the schema but were dead). `subject` is always a string in responses (`""` when unset).

### Clients - `/api/v3/clients` (read only)

| Endpoint | Auth | Request | Response |
|---|---|---|---|
| `GET /clients` | member | query `projectId` (required), `q?`, `limit?`, `offset?` | `data: Client[]` (`id, projectId, email, name, phone, createdAt, updatedAt`) |
| `GET /clients/:id` | member | | `data: Client` (404 for another project's client) |

Clients are created by tickets and inbound mail; the TS create/update/delete routes stay dropped.

## 9. Attachments (`/api/v3/attachments`)

Files live on the server filesystem behind `sd-storage`; the database stores only a `storage_key`. No URLs are stored.

| Endpoint | Auth | Request | Response |
|---|---|---|---|
| `PUT /attachments/:ticketId` | member of the ticket's project | raw body = file bytes; headers `Content-Type` (file type), `X-Filename`, optional `X-Message`, `X-Send-Email: true` | `201 data: { attachmentId, messageId }` |
| `POST /attachments/:ticketId/multiple` | member | multipart: `files` (1..n), `message?`, `sendEmail?` ("true") | `201 data: { attachmentIds: string[], messageId }` |
| `GET /attachments/ticket/:ticketId` | member | | `data: Attachment[]` |
| `GET /attachments/file/:attachmentId` | member (Bearer, or `?token=` signed download token minted for the viewer) | | raw bytes. `Content-Type` from the stored type; `Content-Disposition: inline` only for the allow-listed safe types (png, jpeg, gif, webp, pdf, plain text), `attachment` for everything else; always `X-Content-Type-Options: nosniff`; `Cache-Control: private` |

- Each upload creates an agent message carrying the file(s) (body = `message` or "Sent an attachment: name" / "Sent N attachments"); the message is emailed when `sendEmail` is true and not internal.
- Caps (configurable, defaults): 10 MB per file, 25 MB per request/email, 20 files per request, project quota `storage_quota_mb` (default 1024). Filenames are sanitised (path separators, control chars, leading dots removed, <=255 bytes); the on-disk key is generated (`{projectId}/{ticketId}/{uuid}`), never derived from user input.
- Allowed types: images, PDF, text, office documents, zip (see `ALLOWED_FILE_TYPES` in `apps/web/lib/api/attachments.ts`; the server list is authoritative and the UI list must be a subset). Others: `415`.
- UI note: the mock returned a bare `{ attachmentId }`; the real API wraps it in the envelope, so `attachmentsApi.upload*` must read `.data` (R30).

## 10. Unread (`/api/v3/unread`)

| Endpoint | Auth | Request | Response |
|---|---|---|---|
| `GET /unread/` | user | query `projectId?` | `data: UnreadItem[]` for the caller's projects, oldest first |
| `DELETE /unread/ticket/:ticketId` | member | | `200 data: { deleted: n }` (the mock returned `{}`; the UI ignores the body) |
| `POST /unread/ticket/:ticketId` | member | | `200 data: { created: bool }` marks the ticket unread (`false` if it already was) |

Unread rows are per ticket (not per user), as in the TS product: any member opening the ticket clears it for everyone.

## 11. Viewers (`/api/v3/viewers`)

Presence ("someone else is looking at this ticket"). Rows are touched by heartbeat and expire after 2 minutes (a background job also GCs them).

| Endpoint | Auth | Response |
|---|---|---|
| `GET /viewers/:ticketId` | member | `data: { userId, name }[]` of live viewers (includes the caller if registered) |
| `POST /viewers/:ticketId` | member | `data: { success: true }` - upsert heartbeat for the caller |
| `DELETE /viewers/:ticketId` | member | `data: { success: true }` |

The UI calls POST every ~30 s while a ticket is open and DELETE on close.

## 12. Inbound email webhook (not used by the UI)

`POST /api/v3/incoming-email` - for providers that push mail. Public, but requires a signature:
`X-Webhook-Timestamp` (unix seconds, within 5 min) and `X-Webhook-Signature` = hex HMAC-SHA256 of `"{timestamp}.{raw body}"` with `SD_WEBHOOK_SECRET` (constant-time compare). Replaces TS's bare `X-Webhook-Secret` header (replayable, logged to disk with the secret in `incoming_emails.log`; no logging of payloads now).

Body: `{ from, to, subject?, body?, bodyHtml?, ticketNumber?: number|null, messageId?, inReplyTo?, references?, attachments?: [{ filename?, contentType?, contentBase64 }] }` (TS took attachment `url`s into S3; now inline base64, size-capped).
- Project = the one whose `smtpEmail` equals the lower-cased `to` address (active, non-archived projects only; a global admin must approve the address first (`POST /projects/{id}/mailbox/approve` with `{smtpEmail}` of the reviewed address, `409` if it changed; tenants cannot prove an address themselves); changing the address or servers clears the approval; polling over IMAP needs no proof; unknown => `404`, TS answered 200 with `success:false`).
- `ticketNumber` (or a threading match on `inReplyTo`/`references`) appends to that ticket **of that project only, and only when the sender address equals the ticket client's email or an address already present on the ticket** (ticket numbers are sequential and guessable; anything else opens a new ticket); otherwise a ticket is created. Replayed `messageId` => `200` no-op (dedup via unique message id).
- Auto-replies (`mailer-daemon`, `postmaster`, `Auto-Submitted`) are ignored with `200`.
- Success `201 data: { ticketId, messageId, created: bool }`; `401` bad signature, `422`, `429`.

IMAP polling (per project, `imap_*` settings) produces the same effect without an HTTP endpoint.

## Deviations from the TS product

Fixed on purpose, do not port back:

1. **Envelope.** TS actually returned `{status:"success",data}` / `{status:"error",message,code}` (and, in places, `{success,error:{message}}`), and the UI auth code (`lib/api/auth.ts`, `lib/store.ts`) checks `response.status === "success"`. New contract is `{success,data|error}` throughout. R30 must change those `status` checks to `success`, and the auth `message`/`code` reads to `error.message`/`error.code`.
2. **No tenant isolation in TS.** Any signed-in user could read/patch/delete any ticket, message, project, user, template or email response by id, list all users, and read any project's SMTP password via `/credentials`. Here every query is scoped by membership (404 on foreign ids) and roles are enforced (`project-admin` for settings).
3. **Public ticket create** only needed a `projectId` (guessable UUID, leaks to anyone); now needs the project API key (`X-Project-Key`) plus rate limiting. The widget form must send it (R30).
4. **API key** stored hashed (`api_key_hash`) with a display prefix; full key shown once at create/regenerate. UI `Project.apiKey` becomes `apiKeyPrefix`/one-time `apiKey`.
5. **Credentials endpoint** returned the decrypted SMTP password; now write-only (`hasSmtpPassword`). Added IMAP settings. `smtpHost` no longer defaults to Gmail.
6. **Passwords** were unsalted SHA-256 compared with `===`; now Argon2id. JWT/refresh compare in constant time. Refresh tokens are opaque and hashed; signature comparisons are constant-time.
7. **Refresh** reuse detection kept, with 10 s grace and 90-day family cap. Cookie path/flags unchanged from TS (`refresh_token`, `/api/v3/auth`).
8. **Ticket numbers** were a global `bigserial` (shared across tenants, gaps on rollback). Now per project, gap-free, atomic.
9. **Clients** were global rows shared across tenants (one tenant's edit changed another's). Now per project; unique `lower(email)` per project. The unused `/clients` CRUD is dropped.
10. **Templates** were global; now per project and `canManageTemplates`-gated.
11. **Agent could post as `client`** via `senderType` on `POST /tickets/:id/messages`, bypassing email and reopening tickets. Field removed; agent messages are always `agent`.
12. **`closedAt`** was set on `resolved` but never set for `closed` nor cleared on reopen; fixed.
13. **Search** only covered projects the caller *owned* (members got nothing) and did not bound `limit`; now membership-scoped, `q` min 2 chars, limit capped.
14. **Unbounded pagination** (`limit` unchecked) now capped at 200. Invalid `limit`/`offset` => 422.
15. **Webhook** secret was compared with `!==`, replayable, and payloads (with secret header) appended to a log file; now HMAC + timestamp, no payload logging. "Project not found" is a 404, not a 200.
16. **Soft deletes** (`deletedAt` on users/projects/tickets/messages/clients) are replaced by real deletes with FK cascades; there is no restore feature in the UI.
17. **Health** exposed table names and DB pool config; now `{status, database}` only.
18. **Dropped routes** (UI never calls them): `/clients/*`, `GET /template-responses/:id`, `GET /email-responses/:id`, `GET /attachments/ticket/:id` is kept (used by the viewer), `/test`, `/test-auth`, `/health/db-pool`, `/swagger` redirect.
19. **Error leakage**: TS returned raw `err.message` for 400s (SES token errors, DB messages such as "relation does not exist" were turned into empty lists/503). All 5xx are generic.
20. **Mock vs TS drift resolved**: `POST /tickets/admin` status 201 (mock 200); `GET /tickets/:id/messages` default limit 50 (mock 5, UI always sends limit); delete endpoints return `{success:true,id}` consistently; `DELETE /unread/ticket/:id` returns `{deleted}`.
