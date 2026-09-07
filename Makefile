bullseye:
	@cargo fmt --check && echo "✓ fmt"
	@cargo clippy --quiet --all-targets -- -D warnings && echo "✓ clippy"
	@cargo test --quiet 2>&1 | tail -5 && echo "✓ tests"
	@test -z "$$(git status --porcelain)" && echo "✓ clean" || \
	 (echo "✗ dirty tree"; git status --short; exit 1)

# 🎯T9: benchmarks for the work every tick repeats, locked both ways.
#
#   make bench       run them and print the results
#   make bench-lock  run them and make the result the new baseline
#   make bench-gate  run them and compare against docs/perf/baseline.txt;
#                    fails on a regression AND on an improvement, because an
#                    improvement means the baseline no longer describes the
#                    code and must be re-locked in the same commit
#
# Timing is only comparable on the machine the baseline was recorded on
# (docs/perf/baseline.md names it). Elsewhere, and in CI, use bench-gate-loose,
# which compares only the counted metrics — files parsed, targets returned,
# cron expressions compiled — which are identical on every machine.
bench:
	@cargo bench --quiet --bench bench

bench-lock:
	@cargo bench --quiet --bench bench -- --lock

bench-gate:
	@cargo bench --quiet --bench bench -- --gate

bench-gate-loose:
	@cargo bench --quiet --bench bench -- --gate --loose

.PHONY: bullseye bench bench-lock bench-gate bench-gate-loose
