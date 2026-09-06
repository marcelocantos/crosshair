# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This Is

**Crosshair** is the convergence executor daemon for [bullseye](https://github.com/marcelocantos/bullseye) targets. Bullseye targets can declare a `strategy` block (command, trigger, timeout, retry/backoff). Crosshair reads those targets and runs the strategies on a convergence loop, persisting per-target attempt state in SQLite.

Two binaries, one system. See `README.md` for the relationship to bullseye and `bullseye.yaml` for the convergence targets driving this repo.

## Build and Test

```bash
cargo build          # Build the project
cargo test           # Run all tests
cargo clippy         # Lint
cargo fmt --check    # Check formatting
```

Rust edition 2024.

## Delivery

Merged to master, through a gated push. `scripts/hooks/pre-push` runs
`make gate` (fmt check, clippy `-D warnings`, tests) and refuses the push
when it is red; CI runs the same recipe on master, on `v*` tags and on
pull requests. Wire the hook once per clone with `make hooks`. The owner's
emergency bypass is `git push --no-verify`; agents must not use it.
