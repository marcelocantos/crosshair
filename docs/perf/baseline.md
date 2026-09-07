# crosshair performance baseline (🎯T9)

Crosshair ticks every 30 seconds by default. Each tick re-reads every
configured `bullseye.yaml` and evaluates every strategy-bearing target's
trigger — 2,880 times a day, almost always to discover that nothing
changed and nothing is due. The benchmarks in `benches/bench.rs` measure
exactly that repeated work.

## Running them

```bash
make bench             # run them and print the results
make bench-lock        # make this run the new baseline
make bench-gate        # compare against docs/perf/baseline.txt
make bench-gate-loose  # compare only the counted metrics
```

The gate fails in **both** directions. A regression needs a fix; an
improvement needs `make bench-lock` in the same commit as the change
that earned it, because a baseline nobody re-locks stops describing the
code and the next regression only has to stay under the stale number.

## Timings are floors, not averages

Each benchmark runs many timed rounds and reports the **fastest**. Load
on the machine can only ever make a round slower, so the floor is the
closest thing to an uncontended measurement, and it is the only figure
worth locking on a machine that runs other work. The difference is not
subtle: while a forty-agent fan-out was running on this machine, the
average over the same rounds moved 56% between two back-to-back runs
and the floor moved 1%.

Floors are still only comparable on the machine that recorded them, so
`make bench-gate-loose` compares just the counted metrics — files
parsed, strategy targets found, YAML bytes, cron expressions compiled.
Those are functions of the input and the code and are identical
everywhere.

There is no benchmark framework here on purpose. The harness is one
file; criterion would put plotters and rayon into the dev graph of a
1,700-line crate for numbers a loop can produce.

## Reference machine

| | |
|---|---|
| Machine | Apple M4 Max, macOS 26.6.2, arm64 |
| Toolchain | stable, edition 2024 |
| Recorded | 2026-09-07 |
| Corpus | generated: 24 files, 150 targets each, one in ten carrying a strategy — 702 KB of YAML |

## Locked numbers

`docs/perf/baseline.txt` holds the locked run.

| Benchmark | ns/op floor | counted |
|---|---|---|
| `ReloadFleet` | 0.26 ms | 0 doc-parses, 24 files, 360 strategy targets, 702,348 YAML bytes |
| `EvaluateTriggers` | 0.026 ms | 0 cron-parses, 360 targets |

The two zeros are the numbers to watch. `doc-parses` counts how many
bullseye.yaml documents a reload of an *unchanged* fleet parses, and
`cron-parses` how many cron expressions a pass over warm schedules
compiles. Both are zero because both caches exist, and locking them at
zero is what makes a change that defeats a cache fail the gate rather
than merely make the daemon slower than anyone notices.

## What moved, and by what mechanism

| Benchmark | Before | After | |
|---|---|---|---|
| `ReloadFleet` | 10.52 ms | 0.26 ms | −97.6% |
| `EvaluateTriggers` | 3.29 ms | 0.026 ms | −99.2% |
| cron compiles per pass | 360 | 0 | |

- **A file whose modification time and length are both unchanged is not
  re-read or re-parsed.** The parsed document is kept behind an `Arc`
  and its targets cloned out. mtime-and-length is the same staleness
  pair git's index uses; a content hash would still have to read the
  file, and reading it is most of what we are avoiding. It can miss an
  edit that preserves both — a same-length write inside one filesystem
  timestamp tick — and the consequence is bounded: that one target's
  previous strategy runs until the file is touched again.
- **Compiled cron schedules are cached by expression text.** A cron
  expression is a pure function of its text, so a compiled schedule is
  good for the life of the process. The tick was compiling one per
  strategy-bearing target per interval — on a few hundred targets at the
  default 30-second tick, about a million compiles a day for a handful
  of distinct expressions.

Both caches are pinned by unit tests that assert pointer identity
rather than counter deltas, because the counters are process-wide and
cargo runs tests in parallel. `edited_file_is_reloaded` pins the
property the first cache must not break: crosshair exists to pick up
target edits without a restart.
