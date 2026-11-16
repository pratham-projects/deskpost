# Deskpost (Rust)

Multi-tenant support-ticket inbox: Rust API (Axum/SQLx/Postgres) + Next.js UI. Plan: `docs/PLAN.md`.

## Directives

1. **Caveman mode** in chat. Terse, no filler.
2. **Read `brain/index.md` first.** Navigate index-first via `/path/to/concept.md` links.
3. **Update the brain** after each task: status in `brain/tasks/index.md`, new entry at the top of `brain/log.md`, lessons in `brain/wisdom/`.
4. **Validate**: `python3 ~/.claude/skills/okf/scripts/validate.py brain --strict`.
5. **Git flow**: `main` (prod) <- `staging` <- `feat/*`. Merge `--no-ff` only. Never commit on main/staging. Short messages: `feat: add x`.
6. **No push** until the user approves; local final gate is `make ci` + `make e2e`.
7. Rewrite, not transliteration: read `~/code/sendesk-jobs` for behaviour, fix its bugs, do not port them.

## Commands

```bash
export PATH=/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH
export DATABASE_URL=postgres://pratham@localhost:5433/postgres
make ci
```
