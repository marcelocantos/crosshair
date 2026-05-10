# crosshair

Convergence executor daemon for [bullseye](https://github.com/marcelocantos/bullseye) targets.

## What it does

Bullseye targets describe desired states. Many states converge by mechanical action — push, sync, build, deploy — rather than human design work. Crosshair reads the `strategy` block on a bullseye target and runs the declared command on a convergence loop:

> *Is the target satisfied? If not, run the strategy. Note the outcome. Wait for the next tick.*

Each tick is independent. Stranded work, missed runs, and transient failures all converge automatically on the next tick — no separate "resume retry" state machine.

## Relationship to bullseye

Crosshair and bullseye are two binaries, one system:

- **Shared source of truth.** Crosshair reads `bullseye.yaml` directly. The `strategy` block on a target is the executor's per-target config — no duplicate config file.
- **Shared "is this satisfied?" check.** Each tick, crosshair asks bullseye whether the target is achieved before running the strategy.
- **Shared status surface.** Per-target executor state (`last_attempt_at`, `consecutive_failures`, `cooldown_until`) is queryable alongside bullseye's existing target status.

The repos are separate only because the runtime shapes differ: bullseye is a stateless CRUD-over-YAML MCP server, crosshair is a stateful daemon with a tick loop, SQLite, and launchd integration.

## Status

v0.1 — minimal end-to-end loop in place:

- Loads bullseye.yaml files via `--config` and enumerates targets that carry a `strategy` block.
- Runs each strategy command via `sh -c` with a per-attempt timeout (default 5m, overridable per-strategy).
- Persists `last_attempt_at`, `last_success_at`, `consecutive_failures`, `cooldown_until`, and the most recent stdout/stderr/exit to SQLite.
- Backs off after consecutive failures on a 30m → 2h → 6h → 24h ladder before retrying.
- `crosshair status -c <yaml>` prints one row per strategy-bearing target with its persisted state.

Designed to run as a launchd KeepAlive job. The first proof-point convergence target is yadm dotfile sync, previously driven by `com.marcelocantos.yadm-auto-sync` directly.

See `bullseye.yaml` for the convergence targets driving this repo.

## Usage

```bash
# One-shot — useful for cron-driven setups or testing.
crosshair run --once -c /path/to/bullseye.yaml

# Daemon mode — what launchd runs.
crosshair run -c /path/to/bullseye.yaml --tick 30m

# Inspect state.
crosshair status -c /path/to/bullseye.yaml
```

State defaults to `$HOME/.local/state/crosshair/state.db`; override with `--state`.

## Licence

Apache-2.0.
