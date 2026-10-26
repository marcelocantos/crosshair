bullseye:
	@cargo fmt --check && echo "✓ fmt"
	@cargo clippy --quiet --all-targets -- -D warnings && echo "✓ clippy"
	@cargo test --quiet 2>&1 | tail -5 && echo "✓ tests"
	@test -z "$$(git status --porcelain)" && echo "✓ clean" || \
	 (echo "✗ dirty tree"; git status --short; exit 1)

.PHONY: bullseye

# Publish the formula to the Homebrew tap for an existing release. Run after
# the release assets exist — tapper reads them to build the formula.
release-tap:
	./scripts/release-tap.sh $(TAG)

.PHONY: release-tap
