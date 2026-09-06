.PHONY: gate bullseye hooks

# The delivery gate: the oracles this repo already ran, on the shipped path.
# scripts/hooks/pre-push executes it and refuses the push when it is red, and
# CI runs the same recipe on master, on v* tags and on pull requests.
#
# Deliberately not piped through `tail`: `cargo test | tail -5` reports tail's
# exit status, so a failing test left the recipe green. A gate that cannot go
# red is not a gate.
gate:
	cargo fmt --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo test --workspace

bullseye: gate
	@test -z "$$(git status --porcelain)" && echo "✓ clean" || \
	 (echo "✗ dirty tree"; git status --short; exit 1)

# Once per clone: make git run scripts/hooks instead of .git/hooks. A relative
# path resolves against the worktree the push runs from, so one setting covers
# every `git worktree`.
hooks:
	git config core.hooksPath scripts/hooks
	@echo "core.hooksPath -> scripts/hooks"
