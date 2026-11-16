---
type: Decision
title: ADR-0001 Free email stack
description: Plain SMTP out, IMAP polling and signed webhook in, local filesystem attachments.
tags: [email, storage]
timestamp: 2026-10-27
---

# ADR-0001 Free email stack

**Context**: the original product used Amazon SES (API) and S3, plus a Gmail default SMTP host.

**Decision**: outbound via plain SMTP (any provider, incl. free tiers and Gmail app passwords), inbound via IMAP polling per project and a signed webhook for providers that push. Attachments on the local filesystem behind a storage trait.

**Consequences**: no AWS dependency; works with Mailpit/GreenMail locally; per-project mailbox credentials are encrypted at rest.
