---
type: Gate
title: Pre-Merge Gate
description: Checklist before each merge.
tags: [wisdom, gate]
timestamp: 2026-10-27
---

# Pre-Merge Gate

## feat/* -> staging
1. Task acceptance command passes (see [Tasks](/tasks/index.md)).
2. `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.
3. No violation of [Anti-Patterns](/wisdom/anti-patterns.md).
4. `brain/` updated and validated.
5. Short commit messages; merge with `--no-ff`.

## staging -> main
`make ci`, `make e2e`, review pass with fixes merged. Pushing needs the user's OK.
