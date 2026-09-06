# Changelog

## v0.2.0 — 2026-07-11

Scheduling and hardening release. The v0.1.0 tick loop ran every non-terminal strategy on every tick and could be wedged by a single hung command; both are fixed.

### Added

- **Trigger scheduling** — `needs_attempt` now consults the strategy's `trigger`. `cron:<five-field expression>` runs once in each matching minute (evaluated in UTC), `every:<duration>` runs at most once per duration measured from the previous attempt, and `manual` is never run by the tick loop. An unsupported trigger errors that target and the tick continues.
- **Pull-request CI** — `.github/workflows/ci.yml` runs `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` on the stable toolchain with `Swatinem/rust-cache`.
- **`AGENTS.md`** — canonical instruction file for agentic coding tools.

### Fixed

Remediates the four confirmed findings in `docs/audit/fable-2026-07.md`:

- **Attempts run in their own Unix process group**, killed on timeout, on wait failure, and on drop. A hung command with a pipe-holding grandchild no longer survives its attempt, and a backgrounding command no longer wedges the tick loop.
- **Pipe drains are bounded** on every arm of the attempt, so a descendant that retains the pipe cannot block the loop forever. A drain that times out is reported as a timed-out attempt.
- **SQLite opens with WAL and a five-second busy timeout**, so a concurrent `crosshair status` poll no longer drops an attempt persist.
- **A failed persist keeps its cooldown in memory**, so a transient SQLite error cannot produce a backoff-free re-run on the next tick.

### Changed

- Release workflow actions bumped to Node 24 majors.

## v0.1.0 — 2026-05-10

First release. Crosshair is the convergence executor daemon for [bullseye](https://github.com/marcelocantos/bullseye) targets.

### Added

- **YAML loader** — reads `bullseye.yaml` files via `--config` (repeatable) and enumerates targets that carry a `strategy` block.
- **Command runner with per-attempt timeout** — dispatches `strategy.command` through `sh -c`, captures exit code / stdout / stderr, kills the child if it exceeds the per-strategy `timeout` (default 5m).
- **SQLite-backed state** — keyed by canonical YAML path + target ID. Persists `last_attempt_at`, `last_success_at`, `consecutive_failures`, `cooldown_until`, and the most recent stdout/stderr/exit. Default location: `$HOME/.local/state/crosshair/state.db`.
- **Tick loop with backoff ladder** — every tick reloads configs (so target edits take effect without restart), skips terminal-status and cooldown-active targets, runs the rest, and on failure schedules the next attempt at 30m → 2h → 6h → 24h.
- **`crosshair status` CLI** — joins the live YAML view with the persisted state so targets that have never run yet still appear.
- **`crosshair --help-agent`** — prints the CLI reference followed by the embedded `agents-guide.md` for coding agents.
- **`agents-guide.md`** — domain context for agents: when to use crosshair, strategy schema, tick semantics, launchd setup, gotchas.

### Showcase

The yadm dotfile-sync target (originally driven by `com.marcelocantos.yadm-auto-sync` directly) now converges through crosshair end-to-end: launchd KeepAlive plist runs `crosshair run -c ~/.config/crosshair/targets.yaml --tick 30m`, kill-restart verified transparent, two convergence runs succeeded.
