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
| `ReloadFleet` | see baseline.txt | 24 files, 360 strategy targets, 702,348 YAML bytes |
| `EvaluateTriggers` | see baseline.txt | 360 targets, cron compiles |

`cron-parses` is the one to watch. It counts how many cron expressions
one pass over the fleet compiles, and the whole point of caching
compiled schedules is that this number stops tracking the target count.
