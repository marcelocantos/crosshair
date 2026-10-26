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

## Homebrew

The formula in `marcelocantos/homebrew-tap` is published by
[tapper](https://github.com/marcelocantos/tapper) from the dev Mac, not by CI.
After a release's assets have uploaded (`.github/workflows/release.yml`), run
`make release-tap TAG=vX.Y.Z` (or `tapper push --version vX.Y.Z`). Config lives in `tapper.yaml`;
the tap token is read from the macOS keychain, so no Actions secret is needed.
Docker must be running.

## Delivery

Merged to master.
