# Architecture

(filled in per task; start with `docs/PLAN.md` section 2)

- [Storage](/architecture/storage.md) — sd-storage: trait, fs driver, tokens, quota, TS weaknesses
- [sd-core](/architecture/core.md) — pure logic: status machine, email parsing, threading, reply stripping, secret box, API keys, filename and content-type safety
- [Mail crate](/architecture/mail.md) — SMTP sender, MIME parse, IMAP poller, fake servers
- [DB crate and API contract](/architecture/db-contract.md) — schema, tenant rules, query map
- [API crate](/architecture/api.md) — config, envelope, auth, extractors, limits, how to add a route and its tests
- [Projects and team API](/architecture/projects-api.md) — R21 routes, role matrix, secret and SSRF rules
