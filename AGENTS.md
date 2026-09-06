# AGENTS.md

Crosshair is the convergence executor daemon for [bullseye](https://github.com/marcelocantos/bullseye) targets: it reads `strategy` blocks out of `bullseye.yaml` files and runs their commands on a tick loop, persisting per-target attempt state in SQLite.

This is the canonical instruction file for agentic coding tools. `CLAUDE.md` imports it.

## What This Is

Bullseye targets can declare a `strategy` block (command, trigger, timeout, retry). Crosshair reads those targets and converges them on a loop. Two binaries, one system — see `README.md` for the relationship to bullseye, and `agents-guide.md` for the user-facing domain guide (strategy schema, tick semantics, launchd setup, gotchas).

Rust edition 2024, single crate, no workspace members.

## Build and Test

```bash
cargo check          # Day-to-day feedback loop
cargo build          # Build the binary into ./target
cargo test           # Run all tests (22 as of v0.2.0)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
make bullseye        # The repo gate: fmt + clippy + tests + clean tree
```

CI (`.github/workflows/ci.yml`) runs `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` on every pull request and push to `master`, with `RUSTFLAGS: -D warnings`.

All tests are unit tests inside `src/*.rs` (`#[cfg(test)] mod tests`). There is no `tests/` directory — keep it that way unless a test genuinely needs the binary as a black box, since extra integration binaries cost link time.

## Architecture

The tick loop is the spine; every other module hangs off it.

```
main.rs      → parses CLI, dispatches to tick::run or status::run
  cli.rs     → clap types, --help-agent (embeds agents-guide.md), state-path resolution
  tick.rs    → the convergence loop: reload → filter → run → persist → back off
    loader.rs  → reads YAML files, canonicalises paths, yields StrategyTarget per strategy
      schema.rs  → the subset of bullseye's YAML schema crosshair deserialises
    runner.rs  → trigger evaluation (needs_attempt) and attempt execution (run_attempt)
    store.rs   → SQLite persistence of TargetState, keyed (yaml_path, target_id)
  status.rs  → joins the live YAML view with persisted state and prints a table
```

Load-bearing details:

- **Every tick reloads the configs** (`tick.rs:64`), so YAML edits take effect without a restart. A load failure skips the whole tick rather than aborting the daemon (`tick.rs:66-70`).
- **Targets are keyed by canonical absolute YAML path plus target ID** (`loader.rs:45-54`, `store.rs:102`). Moving a YAML file starts its targets from a clean state.
- **The schema deserialiser ignores unknown fields on purpose** (`schema.rs:3-8`), so crosshair keeps loading newer bullseye files it does not fully understand.
- **An attempt owns a Unix process group** (`runner.rs:131`), killed on timeout, on wait failure, and on drop (`runner.rs:252-256`). This is what stops a backgrounded descendant from wedging the tick loop.
- **Pipe drains are bounded** by `PIPE_DRAIN_TIMEOUT` (`runner.rs:35`) on every arm of the attempt match.
- **A failed persist falls back to an in-memory state map** (`store.rs:57`, `store.rs:190-194`) so backoff survives a transient SQLite error for the life of the process.

Both the process-group and durability behaviours are remediations of confirmed audit findings; do not weaken them without reading `docs/audit/fable-2026-07.md`.

## Conventions

- Apache-2.0. Every source file starts with the copyright line and `SPDX-License-Identifier: Apache-2.0`.
- No magic numbers: durations and caps are named constants (`DEFAULT_TIMEOUT`, `OUTPUT_CAP`, `PIPE_DRAIN_TIMEOUT`, `BACKOFF_LADDER`).
- Module-level `//` comments explain why a module exists and what invariant it holds. Keep them accurate when behaviour changes.
- `bullseye.yaml` in the repo root tracks convergence targets. Write it through the bullseye MCP tools, never by hand.
- `agents-guide.md` is compiled into the binary via `include_str!` (`cli.rs:10`). Editing it changes `crosshair --help-agent` output, so it must stay accurate to the shipped behaviour.
- Documentation of released behaviour lives in `README.md` (overview), `agents-guide.md` (reference), and `CHANGELOG.md` (per-release). Audits live under `docs/`.

## Delivery

Merged to master.
