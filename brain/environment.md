---
type: Environment
title: Local Environment and Git Flow
description: Toolchain, git flow and worker routing for Deskpost.
tags: [environment, git, orca, workers]
timestamp: 2026-10-27
---

# Local Environment and Git Flow

- Rust: brew rustup, `export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH`.
- Postgres 16 on port **5433** (`postgres://pratham@localhost:5433/postgres`, trust auth). Port 5432 is another project. Worker scratch DBs `s1..s4` (Rust) and `j1..j4` (TS repo).
- Web: bun 1.3.14 in `apps/web`. Mailpit via brew. GreenMail jar in `.tools/` for e2e. No docker.
- Git: `main` (prod) <- `staging` <- `feat/*`, `--no-ff`, short commit messages, no push without user approval.
- Workers: Orca orchestration, Claude workers (`--agent claude`), own worktree per `feat/*` branch, coordinator merges.
