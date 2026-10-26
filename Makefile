.PHONY: ci fmt clippy test deny web e2e

ci:
	cargo fmt --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo test --workspace
	cargo deny check
	$(MAKE) web

fmt:
	cargo fmt

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace

deny:
	cargo deny check

web:
	cd apps/web && bun install --frozen-lockfile && bunx tsc --noEmit && bun run build

e2e:
	scripts/e2e.sh
