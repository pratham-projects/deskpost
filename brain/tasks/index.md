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
| R30 | feat/web-live | done, merged |
| R31 | feat/deploy-docs | done, merged |
| R40 | feat/e2e | done (`make e2e`: `scripts/e2e.sh` + `e2e/` Playwright; 5 flows green) |
| R50 | review + fixes | done, merged |
| P3 | feat/deploy | done: deploy verified natively (no docker), `docs/OPERATIONS.md`, `scripts/smoke.sh`, `backup.sh`, `make deploy-check` |
| P4 | feat/webload | done (e2e for every screen; `scripts/load.sh` + `docs/PERFORMANCE.md`) |
| P4-merge | feat/webload | merged to staging (migration renumbered 0006) |
| R60 | final gate | done: staging merged into main locally (2026-11-16); push + tag await approval |
| S2 | feat/site | done, merged (Deskpost site in `site/`) |
| R70 | feat/rebrand | done: SendDesk -> Deskpost full rename + deploy-check env fix; awaiting merge to staging |
| R90 | feat/pre-release | done: binary rename, reopen semantics, LICENSE/CHANGELOG, CI docker + deploy-check, docker verified; awaiting merge to staging |
| R80 | feat/pre-release | done: review findings audit (`reviews/STATUS.md`), `shouldNotify` new-ticket mail (migration 0007) |
| R95 | fix/lettre-advisory | done: lettre 0.11.24, chrono notice ignored, gate green |
| R96 | fix/ci-runner, fix/deploy-check-stat, fix/release-version | done: first real CI green, repo renamed, rc.2 |
| R97 | release/0.1.0 | done: rc.2 validated, v0.1.0 cut |

Rules: [Pre-Merge Gate](/wisdom/pre-merge-gate.md).
