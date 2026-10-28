.PHONY: check
check: fmt lint test

.PHONY: fmt
fmt:
	cargo fmt --all -- --check

.PHONY: lint clippy
lint clippy:
	cargo clippy --all-targets -- -D warnings

.PHONY: test
test:
	cargo test
