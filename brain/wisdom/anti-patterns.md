---
type: AntiPattern
title: Banned Anti-Patterns
description: Rules that must hold in every change to Deskpost.
tags: [wisdom, rules]
timestamp: 2026-10-27
---

# Banned Anti-Patterns

1. Querying without a tenant (project membership) scope.
2. Returning or logging SMTP/IMAP passwords, tokens or webhook secrets.
3. Building a filesystem path from user input.
4. Unbounded queues, bodies, pages or IMAP batches.
5. Committing directly to `main` or `staging`. Merge `--no-ff` from `feat/*` only.
6. Pushing before the local final gate passes and the user approves.
7. Committing `.env`, secrets or `.tools/`.
8. PG18-only SQL (`uuidv7()`). CI pins PG16.
9. Porting a bug from the TypeScript product because it was there.
