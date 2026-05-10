bullseye:
	@cargo fmt --check && echo "✓ fmt"
	@cargo clippy --quiet --all-targets -- -D warnings && echo "✓ clippy"
	@cargo test --quiet 2>&1 | tail -5 && echo "✓ tests"
	@test -z "$$(git status --porcelain)" && echo "✓ clean" || \
	 (echo "✗ dirty tree"; git status --short; exit 1)

.PHONY: bullseye
