# Changelog

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
