// Copyright 2026 Marcelo Cantos
// SPDX-License-Identifier: Apache-2.0
//
// Benchmarks for the work crosshair repeats on every tick (🎯T9), plus
// the gate that locks their numbers in both directions.
//
//     cargo bench --bench bench                     # run them
//     cargo bench --bench bench -- --lock           # re-lock the baseline
//     cargo bench --bench bench -- --gate           # compare to the baseline
//     cargo bench --bench bench -- --gate --loose   # counted metrics only
//
// The gate fails on a regression AND on an improvement. An improvement
// means the baseline no longer describes the code, and letting it
// through silently is how a baseline stops guarding anything: the next
// regression only has to stay under the stale number. So an improvement
// fails too, with the instruction to re-lock in the same commit as the
// change that earned it.
//
// Two kinds of number come out of a run. Timings are reported as the
// FLOOR over many rounds rather than an average, because load on the
// machine can only make a round slower; even so they are only
// comparable against a baseline recorded on the same machine, so they
// carry a wide tolerance and are skipped entirely under --loose.
// Counted metrics — files parsed, targets returned, cron expressions
// compiled — are functions of the input and the code, are identical
// everywhere, and are compared exactly.
//
// There is no benchmark framework here on purpose: the whole harness is
// this file, and adding criterion would put plotters and rayon into the
// dev graph of a 1,700-line crate for numbers a loop can produce.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{TimeZone, Utc};
use crosshair::loader::{StrategyTarget, load_strategy_targets};
use crosshair::runner::needs_attempt;
use crosshair::store::TargetState;

/// How long each benchmark is sampled for before its per-iteration cost
/// is reported. Long enough to swamp timer granularity, short enough
/// that the whole run stays under a minute.
const SAMPLE_TIME: Duration = Duration::from_millis(500);

/// Every benchmark runs at least this many rounds, however slow it is,
/// so its floor is taken over more than a couple of samples.
const MIN_ROUNDS: u32 = 8;

/// The shortest a timed round may be. Iterations are batched up to this
/// so the clock's granularity does not become the measurement.
const MIN_ROUND: Duration = Duration::from_millis(1);

/// Timing tolerance, as a fraction of the baseline. Deliberately wide:
/// a gate that fails on scheduler noise gets switched off, which is
/// worse than a coarse one.
const TIMING_TOLERANCE: f64 = 0.25;

/// The shape of the corpus the benchmarks parse. It follows the owner's
/// real fleet: a few dozen bullseye.yaml files, most targets carrying no
/// strategy, and a minority that do — which is what makes the per-tick
/// reload expensive out of proportion to the work it finds.
const CORPUS_FILES: usize = 24;
const TARGETS_PER_FILE: usize = 150;
const STRATEGY_EVERY: usize = 10;

/// Where the locked baseline lives, relative to the crate root.
const BASELINE: &str = "docs/perf/baseline.txt";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let lock = args.iter().any(|a| a == "--lock");
    let gate = args.iter().any(|a| a == "--gate");
    let loose = args.iter().any(|a| a == "--loose");

    let corpus = Corpus::write();
    let results = run_all(&corpus);

    let rendered = render(&results);
    print!("{rendered}");

    let baseline_path = crate_root().join(BASELINE);
    if lock {
        if let Some(dir) = baseline_path.parent() {
            std::fs::create_dir_all(dir).expect("create docs/perf");
        }
        std::fs::write(&baseline_path, &rendered).expect("write baseline");
        println!("\nlocked {BASELINE} — update docs/perf/baseline.md alongside it");
        return;
    }
    if gate {
        let base = std::fs::read_to_string(&baseline_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", baseline_path.display()));
        if !compare(&parse(&base), &results, loose) {
            std::process::exit(1);
        }
    }
}

// ---------------------------------------------------------------- corpus

/// A generated fleet of bullseye.yaml files on disk, kept alive for the
/// length of the run.
struct Corpus {
    _dir: tempfile::TempDir,
    paths: Vec<PathBuf>,
    bytes: u64,
}

impl Corpus {
    fn write() -> Corpus {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut paths = Vec::with_capacity(CORPUS_FILES);
        let mut bytes = 0;
        for file in 0..CORPUS_FILES {
            let mut yaml = String::from("schema_version: 5\ntargets:\n");
            for n in 0..TARGETS_PER_FILE {
                let id = format!("T{n}");
                write!(
                    yaml,
                    "  {id}:\n    name: target {n} in repo {file}\n    status: converging\n\
                     \x20   acceptance:\n    - the thing is true\n    - and stays true\n\
                     \x20   context: 'Discovered while doing something else entirely.'\n"
                )
                .unwrap();
                if n % STRATEGY_EVERY == 0 {
                    write!(
                        yaml,
                        "    strategy:\n      command: \"make bullseye\"\n\
                         \x20     trigger: \"cron:*/5 * * * *\"\n      timeout: 10m\n"
                    )
                    .unwrap();
                }
            }
            let path = dir.path().join(format!("repo-{file}.yaml"));
            std::fs::write(&path, &yaml).expect("write corpus file");
            bytes += yaml.len() as u64;
            paths.push(path);
        }
        Corpus {
            _dir: dir,
            paths,
            bytes,
        }
    }
}

// ------------------------------------------------------------ benchmarks

fn run_all(corpus: &Corpus) -> Vec<Sample> {
    let targets = load_strategy_targets(&corpus.paths).expect("load corpus");
    // A target crosshair has never attempted: the state every trigger is
    // evaluated against on a fresh daemon, and the one that makes the
    // cron branch do its full work rather than short-circuit.
    let state = TargetState {
        yaml_path: String::new(),
        target_id: String::new(),
        last_attempt_at: None,
        last_success_at: None,
        consecutive_failures: 0,
        cooldown_until: None,
        last_exit_code: None,
        last_stdout: None,
        last_stderr: None,
        last_timed_out: false,
    };
    let now = Utc.with_ymd_and_hms(2026, 9, 7, 10, 5, 0).unwrap();

    vec![
        // The per-tick reload: every configured file, parsed, with the
        // strategy-bearing targets collected. This is what crosshair
        // does before it decides that almost nothing is due.
        bench("ReloadFleet", |_| {
            let loaded = load_strategy_targets(&corpus.paths).expect("load");
            std::hint::black_box(&loaded);
        })
        .with("files", corpus.paths.len() as f64)
        .with("strategy-targets", targets.len() as f64)
        .with("yaml-bytes", corpus.bytes as f64),
        // Trigger evaluation for every strategy-bearing target, which
        // the tick does immediately after the reload.
        bench("EvaluateTriggers", |_| {
            let mut due = 0;
            for t in &targets {
                if needs_attempt(t, &state, now).expect("evaluate") {
                    due += 1;
                }
            }
            std::hint::black_box(due);
        })
        .with("targets", targets.len() as f64)
        .with("cron-parses", cron_parses_per_eval(&targets, &state, now)),
    ]
}

/// How many cron expressions one pass over the fleet compiles. It is a
/// function of the corpus and the code, so it is compared exactly: the
/// point of caching compiled schedules is that this number stops
/// tracking the target count.
fn cron_parses_per_eval(
    targets: &[StrategyTarget],
    state: &TargetState,
    now: chrono::DateTime<Utc>,
) -> f64 {
    let before = crosshair::runner::cron_parse_count();
    for t in targets {
        let _ = needs_attempt(t, state, now);
    }
    (crosshair::runner::cron_parse_count() - before) as f64
}

// --------------------------------------------------------------- harness

/// One benchmark's result: its per-iteration time plus any counted
/// metrics it reports.
struct Sample {
    name: &'static str,
    ns: f64,
    counted: BTreeMap<String, f64>,
}

impl Sample {
    fn with(mut self, metric: &str, value: f64) -> Sample {
        self.counted.insert(metric.to_string(), value);
        self
    }
}

/// Run `f` repeatedly for SAMPLE_TIME and report the FLOOR of the
/// per-iteration cost, not its average.
///
/// Competing load on the machine can only ever make an iteration
/// slower, so the fastest round is the closest thing to an uncontended
/// measurement, and it is the only figure worth locking on a machine
/// that runs other work. An average over the same rounds moved 56%
/// between two back-to-back runs while the floor held.
///
/// Rounds are batched so that a round is long enough to measure: with a
/// batch of one, a benchmark faster than the clock's granularity
/// measures its timer rather than itself.
fn bench(name: &'static str, mut f: impl FnMut(u32)) -> Sample {
    // One untimed pass so a first-call cost (a lazily built regex, a
    // cold page) is not charged to the measurement.
    f(0);

    let probe = Instant::now();
    f(1);
    let single = probe.elapsed().as_nanos().max(1) as f64;
    let batch = (MIN_ROUND.as_nanos() as f64 / single).ceil().max(1.0) as u32;

    let start = Instant::now();
    let mut rounds = 0u32;
    let mut floor = f64::MAX;
    let mut iterations = 0u32;
    loop {
        let round = Instant::now();
        for _ in 0..batch {
            f(iterations);
            iterations += 1;
        }
        let per_iteration = round.elapsed().as_nanos() as f64 / batch as f64;
        floor = floor.min(per_iteration);
        rounds += 1;
        if rounds >= MIN_ROUNDS && start.elapsed() >= SAMPLE_TIME {
            break;
        }
    }
    Sample {
        name,
        ns: floor,
        counted: BTreeMap::new(),
    }
}

/// Render results in the format the baseline file stores: one line per
/// benchmark, the timing first, then counted metrics in name order.
fn render(results: &[Sample]) -> String {
    let mut out = String::new();
    writeln!(out, "# crosshair benchmarks — see docs/perf/baseline.md").unwrap();
    for s in results {
        write!(out, "{}\t{:.0} ns/op", s.name, s.ns).unwrap();
        for (metric, value) in &s.counted {
            write!(out, "\t{value:.0} {metric}").unwrap();
        }
        out.push('\n');
    }
    out
}

/// Parse a rendered baseline back into samples.
fn parse(text: &str) -> Vec<Sample> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split('\t');
        let Some(name) = fields.next() else { continue };
        let name: &'static str = Box::leak(name.to_string().into_boxed_str());
        let mut ns = 0.0;
        let mut counted = BTreeMap::new();
        for field in fields {
            let mut parts = field.split_whitespace();
            let (Some(value), Some(unit)) = (parts.next(), parts.next()) else {
                continue;
            };
            let value: f64 = value.parse().expect("numeric metric");
            if unit == "ns/op" {
                ns = value;
            } else {
                counted.insert(unit.to_string(), value);
            }
        }
        out.push(Sample { name, ns, counted });
    }
    out
}

/// Compare a fresh run against the baseline. Returns false when
/// anything is outside tolerance in either direction.
fn compare(base: &[Sample], got: &[Sample], loose: bool) -> bool {
    let mut ok = true;
    println!();
    for b in base {
        let Some(g) = got.iter().find(|g| g.name == b.name) else {
            println!("{:<20} MISSING from this run", b.name);
            ok = false;
            continue;
        };
        for (metric, want) in &b.counted {
            match g.counted.get(metric) {
                Some(have) if have == want => {}
                Some(have) => {
                    println!(
                        "{:<20} {metric}: locked {want}, measured {have} — a rise is a \
                         regression, a fall must be re-locked in the same commit",
                        b.name
                    );
                    ok = false;
                }
                None => {
                    println!("{:<20} {metric}: not reported by this run", b.name);
                    ok = false;
                }
            }
        }
        if loose {
            continue;
        }
        let delta = (g.ns - b.ns) / b.ns;
        if delta.abs() > TIMING_TOLERANCE {
            println!(
                "{:<20} ns/op: locked {:.0}, measured {:.0} ({:+.1}%) — {}",
                b.name,
                b.ns,
                g.ns,
                delta * 100.0,
                if delta > 0.0 {
                    "a regression"
                } else {
                    "an improvement; re-lock in the same commit (cargo bench -- --lock)"
                }
            );
            ok = false;
        }
    }
    for g in got {
        if !base.iter().any(|b| b.name == g.name) {
            println!("{:<20} NEW — lock it before gating on it", g.name);
        }
    }
    println!(
        "\n{}",
        if ok {
            "bench gate: within tolerance"
        } else {
            "bench gate: FAILED"
        }
    );
    ok
}

/// The crate root, so the baseline path does not depend on the cwd
/// `cargo bench` happens to use.
fn crate_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}
