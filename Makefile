bullseye:
	@cargo fmt --check && echo "✓ fmt"
	@cargo clippy --quiet --all-targets -- -D warnings && echo "✓ clippy"
	@out=$$(cargo test --quiet 2>&1); rc=$$?; printf '%s\n' "$$out" | tail -5; \
	if [ $$rc -ne 0 ]; then echo "✗ tests"; exit $$rc; fi; echo "✓ tests"
	@test -z "$$(git status --porcelain)" && echo "✓ clean" || \
	 (echo "✗ dirty tree"; git status --short; exit 1)

.PHONY: bullseye
