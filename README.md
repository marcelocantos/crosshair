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

Skeleton — no functionality yet. See `bullseye.yaml` for the convergence targets driving this repo.

## Licence

Apache-2.0.
