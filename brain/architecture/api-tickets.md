---
type: Architecture
title: API tickets (R22)
description: Ticket, message, unread, viewer and widget routes, the OutboundMailer seam, and where they differ from the TS product.
tags: [api, tickets, widget, presence, mail]
timestamp: 2026-10-27
---

# API tickets

Code: `crates/api/src/routes/tickets/` (`mod` router + query `Params`, `read`, `create`, `messages`, `unread`, `viewers`, `dto`), `crates/api/src/mailer.rs`. Contract: `docs/API.md` sections 7, 10, 11; general rules in [API crate](/architecture/api.md). One merge line in `lib.rs` (`routes::tickets::router()`), one in `routes/mod.rs`.

- **Visibility** is the sd-db `VISIBLE` join (member and `canViewAllTickets` or assigned/creator). Everything by ticket id answers 404 when invisible, identical to a missing id. Delete adds 403 for plain agents.
- **Lists**: `limit` default 50, clamped to 200, junk or `< 1` is 422; `offset >= 0`. Bad filter values are 422 naming the field. Stats ignore status, priority and assignee. `/search` needs `q` >= 2 chars, literal match over subject, description and all message bodies (internal notes included, members only).
- **Status**: `read::update` runs `sd_core::status::TicketStatus::transition`; closed -> anything but pending is 409, same status is ignored. Assignee must be a project member (FK violation -> 422). Known gap: the check reads the status before the update transaction, so two racing writers could slip a closed ticket past it.
- **Create**: multipart text fields only (`axum` `multipart` feature). File parts are skipped until R23 stores attachments; `sendNotification` is ignored until R23. `/tickets/admin` creates a client message, source `admin`, unassigned. Widget `POST /tickets`: `Rule::Public` limit first (bad keys count), then `X-Project-Key` (hash compared in the create transaction, wrong key, wrong project, unknown project and inactive project all give the same 401 `TOKEN_INVALID`), no cookies read or set, body cap is the global 64 KiB, JSON bodies 415. Client name defaults to the email local part.
- **Messages**: always `senderType: agent`. Notes (`isInternal`) are never mailed, even with `sendEmail: true`. The customer-facing surface is only the widget response and the mail body, neither carries notes.
- **Mailer**: `AppState.mailer: Arc<dyn OutboundMailer>` (`NoopMailer` default, `RecordingMailer` for tests, `RecordingMailer::failing`). `send_reply(&ReplyMail)` runs after the message commits, 30 s timeout. Failure: `tracing::warn` plus an `email_logs` row with status `failed`; the request still answers 201. R23 replaces `state.mailer` and should record `sent` rows itself.
- **Unread**: per ticket, shared by members (contract). Added `POST /unread/ticket/{id}` (mark unread, `{created}`) beside `DELETE` (mark read, `{deleted}`). `/unread` and `/unread/` both work.
- **Presence**: heartbeat row per (ticket, user), live for 120 s, `spawn_presence_gc` purges every 60 s (called from `main serve`); listing ignores expired rows even if the GC lags.

Tests: `tests/tickets.rs`, `ticket_messages.rs`, `ticket_widget.rs`, `ticket_presence.rs`, fixture `tests/tickets_fixture/` (`Env`: owner, agent, global-admin outsider, widget key `KEY`, recording mailer).
