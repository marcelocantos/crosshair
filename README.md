# crosshair

Convergence executor daemon for [bullseye](https://github.com/marcelocantos/bullseye) targets.

## What it does

Bullseye targets describe desired states. Many states converge by mechanical action — push, sync, build, deploy — rather than human design work. Crosshair reads the `strategy` block on a bullseye target and runs the declared command on a convergence loop:

> *Is the target satisfied? If not, run the strategy. Note the outcome. Wait for the next tick.*

Each tick is independent. Stranded work, missed runs, and transient failures all converge automatically on the next tick — no separate "resume retry" state machine.

## Relationship to bullseye

Crosshair and bullseye are two binaries, one system:

- **Shared source of truth.** Crosshair reads `bullseye.yaml` directly. The `strategy` block on a target is the executor's per-target config — no duplicate config file.
- **Shared status source.** Each tick, crosshair reads bullseye's target status and skips terminal targets before evaluating a strategy.
- **Shared status surface.** Per-target executor state (`last_attempt_at`, `consecutive_failures`, `cooldown_until`) is queryable alongside bullseye's existing target status.

The repos are separate only because the runtime shapes differ: bullseye is a stateless CRUD-over-YAML MCP server, crosshair is a stateful daemon with a tick loop, SQLite, and launchd integration.

## Status

v0.1 — minimal end-to-end loop in place:

- Loads bullseye.yaml files via `--config` and enumerates targets that carry a `strategy` block.
- Runs each strategy command via `sh -c` with a per-attempt timeout (default 5m, overridable per-strategy).
- Honors strategy scheduling: five-field `cron:` triggers run once per matching minute, `every:` triggers are rate-limited by the previous attempt, and `manual` triggers never run automatically.
- Places each attempt in its own process group, so timeouts and backgrounded descendants cannot wedge later ticks or survive an attempt.
- Persists `last_attempt_at`, `last_success_at`, `consecutive_failures`, `cooldown_until`, and the most recent stdout/stderr/exit to SQLite.
- Uses SQLite WAL mode and a busy timeout; an in-memory fallback preserves the next-tick cooldown if a transient persist fails.
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

### Trigger scheduling

```yaml
strategy:
  command: /path/to/sync
  trigger: "cron:0 2 * * *" # once at 02:00 UTC each day
  timeout: 2m
```

Crosshair supports `cron:<five-field expression>`, `every:<duration>`, and `manual`. Cron expressions run once in each matching minute; intervals run at most once per duration from the prior attempt; manual strategies are never run by the tick loop.

## Install

```bash
brew install marcelocantos/tap/crosshair
```

Or build from source with `cargo install --path .` from a checkout.

## Quick start for coding agents

Give your coding agent this prompt:

```text
Install crosshair from https://github.com/marcelocantos/crosshair with Homebrew, then run `crosshair --help-agent` and follow the bundled guide to configure a strategy in bullseye.yaml.
```

## For coding agents

If you use an agentic coding tool, include `agents-guide.md` in the project context, or run `crosshair --help-agent` to print the CLI reference and the agent guide together.

## Licence

Apache-2.0.
