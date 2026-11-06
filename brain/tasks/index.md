# Tasks

| id | branch | status |
|----|--------|--------|
| R00 | scaffold | done (coordinator) |
| R10 | feat/db-contract | done, merged |
| R11 | feat/core | done, merged |
| R12 | feat/storage | done, merged |
| R13 | feat/mail | done, merged |
| R20 | feat/api-auth | done, merged |
| R21 | feat/api-projects | done, merged |
| R22 | feat/api-tickets | done, merged |
| R23 | feat/api-mail-attach | done, merged |
| R30 | feat/web-live | done (static export `apps/web/out`; API/Caddy serving left to R31) |
| R31 | feat/deploy-docs | done, merged |
| R40 | feat/e2e | done (`make e2e`: `scripts/e2e.sh` + `e2e/` Playwright; 5 flows green) |
| R50 | review + fixes | web-deploy fixed (fix/web-deploy); other areas per own branch |
| R60 | final gate | todo |

Rules: [Pre-Merge Gate](/wisdom/pre-merge-gate.md).
