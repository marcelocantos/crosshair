# Crosshair entropy audit — 2026-08-22

Full-mode audit (architecture, redundancy, SDLC) plus explicit hygiene
validation. Prior defect audit: [`docs/audit/fable-2026-07.md`](../audit/fable-2026-07.md)
(Fable-5, 2026-07). This is the first entropy-audit report for the repo.

## Executive summary

- **Snapshot:** `/Users/marcelo/work/github.com/marcelocantos/crosshair`
  - Branch: `master` tracking `origin/master`
  - HEAD: `541314febee7538384b5a1f0e93c6c2109a7d0bd`
    (`Harden strategy execution and scheduling (#3)`)
  - Tags at HEAD: `v0.2.0`
  - Date: 2026-08-22
  - Initial dirty state (`git status --porcelain=v1 -b`):
    ```
    ## master...origin/master
    ?? .claudia-mcp-home/
    ```
    Untracked `.claudia-mcp-home/` is user-owned local agent state; it was
    not modified, staged, or discarded.
- **Scope:** the single crate `crosshair` (Rust edition 2024), CLI/daemon
  entry points, SQLite store, GitHub Actions, Makefile, agent/docs
  surfaces, and `bullseye.yaml`. Exclusions below.
- **Headline mechanism:** a small, acyclic-enough daemon with a clean
  YAML→runner→SQLite loop, but **cron due-ness is sampled only against
  the current UTC minute** while the documented launchd invocation uses
  `--tick 30m` and a sequential per-attempt timeout whose default is 5
  minutes. Missed matching minutes are never caught up. That is a
  scheduler, not a code-style, problem.
- **Highest-consequence findings:**
  - **ENT-001 (P1):** `cron:` triggers + documented `--tick 30m` (and any
    tick that overruns a matching minute) skip the window with no
    catch-up.
  - **ENT-002 (P2):** invalid `strategy.timeout` silently becomes 5m.
  - **ENT-003 (P2):** `retry` is deserialized and shown to operators as
    config, but the backoff ladder is hard-coded.
  - **ENT-004 (P2):** crate/tag `0.2.0` shipped; README and CHANGELOG
    still describe v0.1.0.
- **Unverified residue:** bullseye schema field-parity (other repo, out
  of scope); Homebrew `skip_checksum` semantics; whether any live
  launchd job actually combines `--tick 30m` with `cron:`; live KeepAlive
  journey; advisory-db / SBOM scan (tools not installed).

Fable F1–F4 (process-group kill, bounded pipe drain, WAL + busy timeout
+ in-memory cooldown fallback) are closed in source and covered by
in-crate tests. They are not re-opened.

## Scope and exclusions

**In scope**

- `src/*.rs` (library + `src/main.rs` binary)
- `Cargo.toml` / `Cargo.lock`
- `.github/workflows/{ci,release}.yml`, `Makefile`
- `README.md`, `agents-guide.md`, `AGENTS.md`, `CLAUDE.md`, `CHANGELOG.md`,
  `bullseye.yaml`, `LICENSE`, `.gitignore`
- Prior audit `docs/audit/fable-2026-07.md`

**Named exclusions (not silent omissions)**

- `target/` — cargo build cache / artifacts
- `.git/`
- Untracked `.claudia-mcp-home/` — user-owned; listed only as a gitignore
  gap (ENT-009)
- The bullseye repository and its YAML schema implementation — this audit
  does not leave the crosshair working tree
- Homebrew tap `marcelocantos/homebrew-tap` (release consumer, not this
  tree)
- Generated lockfile contents beyond duplicate/direct-dep inspection

No generated, vendored, or fixture trees exist under `src/`. Tests live
inside `#[cfg(test)]` modules in the same files as production code.

## Commands run

Toolchain: `rustc 1.96.0 (ac68faa20 2026-05-25)` (Homebrew),
`cargo 1.96.0 (30a34c682 2026-05-25)`, `rustfmt 1.9.0`,
`clippy 0.1.96`. Working directory: repo root. `cargo-nextest`,
`cargo-machete`, `cargo-deny`, and `cargo-audit` are not installed;
they were not added.

| Command | Exit | Shipped vs auxiliary | Result / limitation |
|---|---|---|---|
| `git rev-parse --abbrev-ref HEAD && git rev-parse HEAD && git status --porcelain=v1 -b` | 0 | provenance | `master` / `541314fe…` / `?? .claudia-mcp-home/` |
| `cargo fmt --check` | 0 | shipped (CI step) | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 | shipped (CI step) | clean; does **not** enable `unused_crate_dependencies` |
| `cargo test --workspace` | 0 | shipped (CI step) | **22 passed**, 0 failed, 0 ignored (lib unittests). `src/main.rs` and doc-tests: 0 tests. Sequential `--test-threads=1` used here; CI omits that flag. |
| `cargo tree --locked -e normal --depth 1` | 0 | auxiliary | lockfile in sync with `Cargo.toml` at this snapshot |
| `cargo tree --duplicate` | 0 | auxiliary | `hashbrown` 0.16.1 (via rusqlite) and 0.17.0 (via serde_yaml_ng/indexmap) |
| `cargo tree -i thiserror --depth 1` / `-i serde_json` | 0 | auxiliary | both are **direct** deps of `crosshair` only |
| `/Users/marcelo/.claude/skills/hygiene/hygiene_check.py` | 1 | hygiene skill | `FileNotFoundError: …/hygiene.yaml` — posture not declared |
| `diff -u AGENTS.md CLAUDE.md` | 1 (diff) | auxiliary | identical except H1 / tool-name sentence |
| `ls rust-toolchain.toml rust-toolchain .cargo hygiene.yaml .github/dependabot.yml` | 2 | auxiliary | all absent |

Limitations: no nextest (declared CI path is `cargo test`); no advisory
scan; no `--release` rebuild; no launchd execution; clippy cannot decide
unused crates without an extra lint or `cargo-machete`.

## Observed architecture

Declared (README / agents-guide / `bullseye.yaml` T1): two binaries, one
system. Bullseye YAML is the target source of truth; crosshair is a
stateful executor with a tick loop, SQLite, and launchd KeepAlive.
Satisfaction in v0.1/v0.2 is “trust the YAML `status` field”; a pluggable
check is planned, not present.

Observed data flow:

```
CLI (clap) ── run ──► tick::run
                 │         │
                 │         ▼
                 │    load_strategy_targets (canonicalize + parse subset YAML)
                 │         │
                 │         ▼
                 │    converge_one ── needs_attempt ── run_attempt (sh -c, process group)
                 │         │
                 │         ▼
                 │    Store::record_attempt (SQLite WAL)
                 │
                 └── status ──► load_strategy_targets + Store::get_or_empty
```

**Entry points**

- Binary: `src/main.rs` (`crosshair run`, `crosshair status`,
  `--help-agent`).
- Library modules: `cli`, `loader`, `schema`, `runner`, `store`, `tick`,
  `status`.
- Deployable: single binary; Homebrew formula from the release workflow;
  launchd plist is documentation, not a tracked file.

**Directional dependencies (same crate)**

- `main` → `cli`, `tick`, `status`
- `tick` → `cli`, `loader`, `runner`, `store`
- `status` → `cli`, `loader`, `store`
- `loader` → `schema`
- `runner` → `loader`, `schema`, **`store::TargetState`**
- `store` → `loader`, **`runner::AttemptOutcome`**

There is a two-node module cycle `runner` ↔ `store`. It compiles because
both are submodules of one crate. `needs_attempt` (scheduling) lives in
`runner` only because it takes `TargetState`; `record_attempt` takes
`AttemptOutcome`. The cycle is real and small; it is not a finding by
itself.

**Public integration surfaces**

- CLI flags `--config` (repeatable), `--state`, `--tick`, `--once`
- Bullseye YAML subset (`schema::Doc` / `Strategy`)
- SQLite file default `$HOME/.local/state/crosshair/state.db`, keyed by
  `(yaml_path, target_id)`
- `sh -c` as the authority-crossing execution surface (intentional)

**Declared vs observed**

| Rule | Status |
|---|---|
| YAML is the target SoT; SQLite holds executor state | declared and observed |
| Extra YAML fields ignored | declared and tested (`schema::tests::ignores_unknown_fields`) |
| Terminal statuses never run | declared, observed, tested |
| Process-group kill + bounded drain | declared (post-Fable), observed, tested |
| WAL + busy_timeout + persist-failure fallback | declared, observed, tested |
| `cron:` runs once per matching minute | declared; **observed as current-minute sample only** (ENT-001) |
| `retry.*` controls backoff | declared as “advisory” in the agent guide; **observed as unread** (ENT-003) |
| Daemon tick 30m | declared in README/launchd sample; **contradicts minute-resolution cron** |

**Unknown intent (owner residue):** whether missed cron minutes should
be skipped (classic cron) or caught up (anacron / “due since last
attempt”). The documented 30m daemon tick is incompatible with
current-minute matching either way.

## Dimension vector

First entropy-audit snapshot. “Change from baseline” is relative to the
July Fable defect audit where that audit spoke to the dimension; otherwise
n/a.

| Dimension | State | Evidence summary | Change from baseline |
|---|---|---|---|
| Architecture topology | healthy | Single crate, 9 focused modules, one binary, YAML→tick→SQLite. Module cycle runner↔store is local. Sequential tick is an explicit loop, not a hidden graph. | Fable topology unchanged; process/store boundaries repaired |
| Redundancy / sources of truth | concern | Subset YAML schema vs bullseye; `retry` vs `BACKOFF_LADDER`; two `truncate` helpers already drifted; README/CHANGELOG vs `0.2.0`; AGENTS.md≈CLAUDE.md | New vs Fable (Fable did not inventory SoT) |
| Change amplification | healthy | ~1.7k LOC of Rust; co-change of `runner`/`store`/`tick` is the natural cluster. Schema evolution has no migration story (ENT-011) but there is only one table version in the wild. | — |
| Local code quality | concern | Clippy/fmt clean. Dead crates `thiserror`/`serde_json`. `status::truncate` is not char-boundary safe. `resolved_timeout` swallows parse errors. | Improved process/store code after Fable; leftover quality nits |
| Correctness / verification | concern | 22 tests cover Fable regressions, backoff, cron/interval math with **injected** `now`. No test that the ticker + `--tick 30m` can hit a cron minute. No binary/launchd journey. | F1–F4 closed; scheduler catch-up untested |
| Security / dependencies | concern | `sh -c` is the product. No `cargo-deny`/`cargo-audit`/dependabot. CI not `--locked`. `.gitignore` covers `.claude/` but not `.claudia-mcp-home/`. | CI added in #3; lock enforcement only on release |
| Build / release / operations | concern | CI: fmt + clippy `-D warnings` + test + rust-cache (ubuntu-latest). Release: `--locked` matrix, Homebrew tap, `skip_checksum: true`, aarch64-linux tests skipped. Makefile `bullseye` gate duplicates CI plus dirty-tree. | CI did not exist at v0.1.0 |
| Documentation / governance | concern | Agent guide embedded via `include_str!` (good). README still “v0.1”. CHANGELOG stops at v0.1.0. `hygiene.yaml` absent. No CODEOWNERS / dependabot / SECURITY.md. | AGENTS.md added in #3 as a copy of CLAUDE.md |

Do not collapse the vector to a scalar.

## Findings

### ENT-001: Cron due-ness is a current-minute sample with no catch-up

- **Priority:** P1
- **Dimensions:** Correctness / verification; Architecture topology
- **Status:** observed fact
- **Evidence:**
  - `src/runner.rs:69-89` — `cron:` parses a five-field expression,
    truncates `now` to the current UTC minute, and returns false unless
    `Cron::is_time_matching(&current_minute)`. There is no “most recent
    matching instant ≤ now since `last_attempt_at`” check.
  - `src/tick.rs:55-59` — daemon loop is `tokio::time::interval` with
    `MissedTickBehavior::Delay`. Phase equals process start. A long
    `run_one_tick` delays the next sample by a full `--tick` after the
    overrun, rather than catching up.
  - `src/tick.rs:77-81` — `now` is captured once per tick (good for
    intra-tick consistency); it does not walk skipped minutes.
  - `src/cli.rs:66-68` — default `--tick` is `30s`.
  - `README.md:46-47` — documented daemon invocation is
    `crosshair run -c … --tick 30m`.
  - `README.md:57-64` — the same document’s trigger example is
    `trigger: "cron:0 2 * * *"` (once at 02:00 UTC).
  - `agents-guide.md:50` — “`--tick` … determines how often crosshair
    checks whether each strategy’s trigger is due.”
  - `agents-guide.md:83` — launchd sample `ProgramArguments` includes
    `--tick` `30m`.
  - `src/runner.rs:432-444` — `daily_cron_runs_once_per_day_across_ticks`
    injects `now` at 00:00:15 and +1 day; it never drives the ticker.
- **Mechanism:** with `--tick 30m`, ticks fire at `start, start+30m, …`.
  Hitting minute `02:00` requires the daemon’s start phase to land in
  that minute. launchd `RunAtLoad` / `KeepAlive` restart time is not
  aligned to cron. Independently, a sequential attempt whose timeout
  defaults to 5 minutes (`src/runner.rs:28-29`) can push the next tick
  past the matching minute even when `--tick` is `30s`. Missed windows
  wait until the expression matches again (tomorrow, for a daily cron).
  `every:` is unaffected because it uses elapsed time from
  `last_attempt_at`.
- **Blast radius:** every `cron:` strategy under the documented launchd
  command line; T2’s original consumer (scheduled ingest that must not
  run every tick) is the use case this code was added for.
- **Counterevidence checked:** T2 acceptance wants “only when due, not
  every tick,” which current-minute matching satisfies *when sampled in
  the matching minute*. Classic cron also skips downtime. That does not
  justify a 30-minute sampler against a 1-minute grid, nor documenting
  both in one README. `every:` + 30m tick remains coherent.
- **Smallest coherent remediation:** evaluate cron as “a matching
  instant exists in `(last_attempt_at, now]`” (croner’s next/prev fire),
  still once per matching minute. Until that lands, fail or warn when
  `--tick` > 1m and any loaded strategy is `cron:`, and change the
  launchd sample to `30s` (or `1m`).
- **Verification:** (1) unit: `needs_attempt` at 02:30 with
  `cron:0 2 * * *` and `last_attempt_at` yesterday still returns true
  after catch-up; (2) loop: a fake clock advancing 30m steps from an
  08:17 start must still fire `0 2 * * *` once that day. The current
  tests would stay green without (2).
- **Ratchet candidate:** architecture test that loads the README/agent
  launchd snippet’s `--tick` and every `cron:` example and asserts the
  sampler can hit those expressions; or a clippy-unrelated CI script
  grepping the launchd example against a documented maximum tick.

### ENT-002: Unparseable `strategy.timeout` silently becomes 5 minutes

- **Priority:** P2
- **Dimensions:** Correctness / verification; Local code quality
- **Status:** observed fact
- **Evidence:** `src/runner.rs:105-112` uses
  `humantime::parse_duration(s).ok().unwrap_or(DEFAULT_TIMEOUT)`.
  `--tick` parse failures abort (`src/tick.rs:38-39`). Unsupported
  triggers bail (`src/runner.rs:102`). Timeout typos do not.
  Documented in the Fable trust-boundary list; still live. No test
  asserts reject-vs-fallback.
- **Mechanism:** `timeout: 2min` / `2 minutes` / `2` is accepted as 5m.
  Operators believe they tightened the hang ceiling; the 9.5h-hang class
  of bug this daemon exists to bound is only bounded at the default.
- **Blast radius:** every strategy with a malformed timeout string.
- **Counterevidence checked:** unknown fields are ignored by design;
  timeout is a *known* field whose value is discarded. Default 5m is
  documented for *absence*, not for parse failure.
- **Smallest coherent remediation:** `bail!` (or a traced error that
  skips the target) on parse failure, same as `--tick` and bad cron.
- **Verification:** unit test `timeout: "2min"` → `needs_attempt` /
  `run_attempt` setup returns error, not a 5m run.
- **Ratchet candidate:** test plus, optionally, a schema validator that
  round-trips `humantime` on load.

### ENT-003: `retry` is a second, unread, backoff authority

- **Priority:** P2
- **Dimensions:** Redundancy / sources of truth; Documentation / governance
- **Status:** observed fact
- **Evidence:**
  - `src/schema.rs:53-63` — `retry.max_attempts` and `retry.backoff`
    deserialize.
  - `src/schema.rs:109-119` — test asserts they parse.
  - `rg retry` over `src/*.rs` — no read in `tick` or `runner` except
    struct literals in tests.
  - `src/tick.rs:30-35` — `BACKOFF_LADDER` is `30m, 2h, 6h, 24h`.
  - `agents-guide.md:30-31` — “advisory in v0.1; cooldown ladder is
    fixed.”
- **Mechanism:** YAML authors can write `max_attempts: 5` /
  `backoff: exponential` and observe a different schedule. Two sources
  of truth for the same policy; they can (and do) disagree. The field
  is not dead in the schema, so it will not fail clippy.
- **Blast radius:** every strategy that sets `retry`; future readers of
  bullseye YAML who assume the nested block is executable.
- **Counterevidence checked:** the agent guide discloses “advisory.”
  README’s strategy example omits `retry`. Disclosure does not stop the
  silent no-op.
- **Smallest coherent remediation:** either honor `retry` (even a
  reduced subset) or stop deserializing it and reject unknown
  `strategy.retry` until implemented. Do not keep a parsed no-op.
- **Verification:** if rejected, a fixture with `retry:` fails load; if
  honored, changing `max_attempts` changes `next_cooldown` in a test.
- **Ratchet candidate:** grep/test that `RetryPolicy` fields are read
  from `next_cooldown` / `needs_attempt`, or that serde `deny_unknown_fields`
  is on `Strategy` once bullseye compatibility is versioned.

### ENT-004: Shipped 0.2.0 with v0.1.0 release notes

- **Priority:** P2
- **Dimensions:** Documentation / governance; Build / release / operations
- **Status:** observed fact
- **Evidence:**
  - `Cargo.toml:3` — `version = "0.2.0"`
  - `git tag -l` — `v0.1.0`, `v0.2.0` (v0.2.0 == HEAD `541314fe`)
  - `CHANGELOG.md` — only `## v0.1.0 — 2026-05-10`; no 0.2.0 section
  - `README.md:25` — “v0.1 — minimal end-to-end loop in place”
  - `agents-guide.md:105` — “Limits in v0.1”
  - v0.1.0..v0.2.0 includes process-group kill, WAL, trigger scheduling,
    and CI — the work T2/T3/T4 describe
- **Mechanism:** operators and agents reading the shipped docs will not
  see trigger scheduling, process-group guarantees, or CI as released
  behavior. `/release` consumers of CHANGELOG skip the 0.2.0 delta.
- **Blast radius:** Homebrew users, coding agents via `--help-agent`,
  humans following README.
- **Counterevidence checked:** `agents-guide.md` *body* does describe
  cron/every/manual and process groups; the version labels are what
  drifted. `include_str!` means the stale “v0.1” heading ships in the
  binary.
- **Smallest coherent remediation:** add a CHANGELOG 0.2.0 section from
  `#3`, retitle README status to v0.2, rename the agent-guide limits
  heading.
- **Verification:** `rg -n 'v0\\.1' README.md CHANGELOG.md agents-guide.md`
  is empty or historically qualified; CHANGELOG has a `## v0.2.0` heading
  matching `Cargo.toml`.
- **Ratchet candidate:** release skill / CI check that
  `CHANGELOG.md` contains the crate version.

### ENT-005: Two `truncate` helpers; the status CLI one can panic

- **Priority:** P2
- **Dimensions:** Redundancy / sources of truth; Local code quality; Correctness / verification
- **Status:** observed fact (panic); inference (operator impact)
- **Evidence:**
  - `src/status.rs:36` calls `truncate(&t.target_id, 24)`
  - `src/status.rs:94-99` — `format!("{}…", &s[..max - 1])` with no
    `is_char_boundary` guard
  - `src/store.rs:247-248, 274-283` — store truncation walks back to a
    char boundary and appends `…[truncated]`
- **Mechanism:** `&str` indexing panics if `max-1` is mid-character.
  Target IDs are strings with no charset constraint in `schema.rs`.
  A 25+-byte ID whose byte 23 is mid-UTF-8 crashes `crosshair status`.
  The store path already solved this; the copies drifted.
- **Blast radius:** status CLI only; tick/store path is safe. Typical
  bullseye IDs (`T1`, `T4`) are ASCII and will not trip it.
- **Counterevidence checked:** no production call site uses
  `status::truncate` on stdout/stderr (those go through store). No test
  covers a multibyte ID.
- **Smallest coherent remediation:** one helper (or `chars().take()`),
  used by status and store; add a multibyte test.
- **Verification:** `truncate("é".repeat(20), 24)` must not panic and
  must return valid UTF-8.
- **Ratchet candidate:** that unit test.

### ENT-006: One unreadable config aborts every config for the tick

- **Priority:** P2
- **Dimensions:** Build / release / operations; Correctness / verification
- **Status:** observed fact
- **Evidence:** `src/loader.rs:57-58` — `load_doc(path)?` inside the
  merged-files loop. `src/tick.rs:64-69` — any `Err` logs “failed to
  load configs; skipping tick” and returns, so **no** strategy runs
  that tick. Canonicalisation errors (`src/loader.rs:47-50`) take the
  same path. `status::run` propagates the error to process exit
  (`src/status.rs:17-18`).
- **Mechanism:** `--config a.yaml --config b.yaml` with a transient
  permission/parse error on `a.yaml` starves `b.yaml` until the next
  successful load. A persistent parse error disables the whole daemon
  loop without running the still-valid file.
- **Blast radius:** multi-file deployments (CLI documents repeatable
  `-c`). Single-file setups fail closed, which may be desired.
- **Counterevidence checked:** loader comment (`src/loader.rs:41-44`)
  says the caller decides skip vs abort; tick chooses skip-all.
  Fail-closed for a single required file is reasonable; fail-all-files
  is the defect.
- **Smallest coherent remediation:** per-file error isolation in
  `load_strategy_targets` (or in `run_one_tick`): emit a warning, keep
  targets from files that loaded.
- **Verification:** two temp YAMLs, second invalid, `--once` still
  records an attempt for the first.
- **Ratchet candidate:** that integration test.

### ENT-007: PR CI does not `--locked` while release does

- **Priority:** P2
- **Dimensions:** Security / dependencies; Build / release / operations
- **Status:** observed fact
- **Evidence:**
  - `.github/workflows/ci.yml:20-22` — `cargo fmt --check`,
    `cargo clippy --workspace --all-targets -- -D warnings`,
    `cargo test --workspace` (no `--locked`)
  - `.github/workflows/release.yml:41-44` — `cargo build --release --locked`
    and `cargo test --release --locked`
  - `Makefile:1-4` — fmt/clippy/test without `--locked`
- **Mechanism:** a PR that edits `Cargo.toml` without refreshing
  `Cargo.lock` still goes green on `master` CI (cargo may resolve on
  the runner). The published artifact job then fails or, worse, if
  someone ships without the release job, the lockfile is not the build
  input. Supply-chain review of `Cargo.lock` is not a PR gate.
- **Blast radius:** every PR; release job is the backstop for tags.
- **Counterevidence checked:** `Cargo.lock` is committed; `cargo tree --locked`
  succeeds on this snapshot. rust-cache is present. The gap is the
  missing flag, not an absent lockfile.
- **Smallest coherent remediation:** add `--locked` to CI clippy/test
  (fmt does not need it). Mirror on `make bullseye`.
- **Verification:** a PR that bumps a crate version in `Cargo.toml`
  only must fail CI.
- **Ratchet candidate:** the CI YAML line itself.

### ENT-008: Direct crates `thiserror` and `serde_json` have no source users

- **Priority:** P3
- **Dimensions:** Local code quality; Security / dependencies
- **Status:** observed fact
- **Evidence:** `Cargo.toml:22-23` declare `serde_json` and `thiserror`.
  `rg thiserror` / `rg serde_json` over `*.rs` — no matches.
  `cargo tree -i` shows each is requested only by `crosshair`.
  Clippy `-D warnings` still passed.
- **Mechanism:** unused crates still compile, still enter the lockfile,
  and hide future “we thought we had typed errors / JSON I/O” intent.
  `StrategyTarget::key` (`src/loader.rs:24-30`) is similarly unread.
- **Blast radius:** build graph only; no runtime.
- **Counterevidence checked:** not a transitive-only dep; not used via
  macros in this crate. `Store::list` is `pub` but only called from
  store tests — weaker, left as inventory not a separate ENT.
- **Smallest coherent remediation:** remove the two deps (and
  `key()` if still unused).
- **Verification:** `cargo machete` or `#![deny(unused_crate_dependencies)]`
  in `lib.rs` / `main.rs`.
- **Ratchet candidate:** `unused_crate_dependencies` in CI clippy, or
  `cargo machete` once adopted fleet-wide.

### ENT-009: `.gitignore` covers `.claude/` but not `.claudia-mcp-home/`

- **Priority:** P3
- **Dimensions:** Security / dependencies; Documentation / governance
- **Status:** observed fact
- **Evidence:** `.gitignore:8-9` ignores `.claude/`. Working tree at
  audit start: `?? .claudia-mcp-home/`. That tree is not in the
  repository; a naive `git add .` would stage it.
- **Mechanism:** local agent/MCP state (including credential-shaped
  files in some layouts) is one `git add` away from the origin. The
  sibling `.claude/` directory was already recognized as un-committable.
- **Blast radius:** next careless add/commit; not currently tracked.
- **Counterevidence checked:** nothing under `.claudia-mcp-home/` is
  staged or committed. User-owned; this audit did not inspect contents.
- **Smallest coherent remediation:** ignore `.claudia-mcp-home/` next to
  `.claude/`.
- **Verification:** `git check-ignore -v .claudia-mcp-home` succeeds.
- **Ratchet candidate:** a `file:` hygiene item or a pre-commit name
  check, once hygiene is declared.

### ENT-010: Bullseye schema is an unversioned subset; `schema_version` is unread

- **Priority:** P3
- **Dimensions:** Redundancy / sources of truth; Change amplification
- **Status:** observed fact (unread field); inference (cross-repo drift)
- **Evidence:** `src/schema.rs:1-8, 16-19, 47-55` — comment says the
  subset mirrors bullseye 0.25+ field names; `schema_version` is
  deserialized with a default and never consulted outside tests.
  Tests and tick fixtures use `schema_version: 3`; this repo’s
  `bullseye.yaml:13` is `schema_version: 5`. Unknown fields are ignored
  (`src/schema.rs:91-120`).
- **Mechanism:** a bullseye rename or semantic change of `strategy.*`
  or `status` can load “successfully” with wrong defaults (`Status`
  defaults to `Identified`, which is runnable). There is no contract
  test against bullseye’s schema crate/file (and this audit did not
  open that repo).
- **Blast radius:** every loaded YAML; grows with bullseye releases.
- **Counterevidence checked:** ignoring unknown fields is deliberate
  forward-compat. Required fields (`name`, `command`, `trigger`) still
  fail serde if missing. The unread `schema_version` is the missing
  ratchet, not the subset itself.
- **Smallest coherent remediation:** define a supported
  `schema_version` range and warn/fail outside it; add a fixture copied
  from a frozen bullseye example (vendored, version-pinned).
- **Verification:** loading `schema_version: 1` with a known-incompatible
  shape fails loudly.
- **Ratchet candidate:** that fixture test.

### ENT-011: SQLite schema is `CREATE TABLE IF NOT EXISTS` with no user_version

- **Priority:** P3
- **Dimensions:** Change amplification; Build / release / operations
- **Status:** observed fact
- **Evidence:** `src/store.rs:88-107` — single `CREATE TABLE IF NOT EXISTS
  target_state`. `configure_connection` (`src/store.rs:220-223`) sets
  busy timeout + WAL only. No `PRAGMA user_version`, no ALTER path.
- **Mechanism:** adding a column does not migrate existing
  `~/.local/state/crosshair/state.db` files; subsequent `SELECT` lists
  of new columns will error or stay unpopulated depending on the
  statement. v0.1→v0.2 did not change columns (WAL is a pragma), so this
  has not bitten yet.
- **Blast radius:** every installed state DB at the next schema change.
- **Counterevidence checked:** parameterized SQL, explicit column lists
  (no `SELECT *`), WAL, busy timeout — all sound for the current shape.
  sql.md’s `PRAGMA optimize` is unused; the table is tiny, not raised
  as a finding.
- **Smallest coherent remediation:** set `user_version` now (while still
  1) so the next ALTER has a branch.
- **Verification:** open a v1 file after a v2 binary starts; columns
  exist.
- **Ratchet candidate:** a test that copies a checked-in v1 DB and
  opens it on `Store::open`.

### ENT-012: No shipped-binary or launchd journey; CI is Ubuntu-only

- **Priority:** P3
- **Dimensions:** Correctness / verification; Build / release / operations
- **Status:** observed fact
- **Evidence:** all 22 tests are `#[cfg(test)]` inside the library.
  `src/main.rs` has zero tests. CI (`.github/workflows/ci.yml:15`) is
  `ubuntu-latest` only. Release tests skip `aarch64-unknown-linux-gnu`
  (`.github/workflows/release.yml:43-44`). Process-group tests are
  `#[cfg(unix)]` and therefore *do* run on Linux CI. T1 acceptance
  (“daemon runs as a launchd KeepAlive job”) is a one-machine proof
  recorded in bullseye, not a CI oracle.
- **Mechanism:** clap parsing, `--help-agent`, default state-path
  (`HOME`), and launchd spawn/restart are unverified on the shipped
  binary. A regression that only appears in `main` or on Darwin
  process-group semantics would not fail the 22 tests.
- **Blast radius:** release and macOS operators; Linux CI still
  exercises the unix kill path.
- **Counterevidence checked:** in-crate tests spawn real `sh` and
  grandchildren — they are not fakes. That is the right oracle for
  F1–F3. It is not a journey in the `journeys.md` sense (whole binary +
  plist + KeepAlive).
- **Smallest coherent remediation:** one `crosshair run --once -c <tmp>
  --state <tmp>` test of the binary (`assert_cmd` / `trycmd`), plus an
  explicit exception for launchd (manual / periodic) rather than citing
  T1 as CI evidence.
- **Verification:** break `Cli` default state path; the journey fails.
- **Ratchet candidate:** that binary `--once` test in CI.

## Redundancy and competing-source-of-truth inventory

| Fact | Authorities | Drift already? | Action |
|---|---|---|---|
| Target list + strategy command/trigger | bullseye YAML (SoT, declared); `schema.rs` subset parser | unknown vs bullseye repo | ENT-010 |
| Backoff schedule | `BACKOFF_LADDER`; YAML `retry`; agent-guide “advisory”; T1 context text omits 24h | yes (retry unread; T1 text) | ENT-003 |
| Output truncation | `status::truncate`; `store::truncate` | yes (char-boundary) | ENT-005 |
| Product version | `Cargo.toml` / git tag; README; CHANGELOG; agent-guide headings | yes | ENT-004 |
| Agent instructions | `AGENTS.md`; `CLAUDE.md` (identical except H1) | headings only | retain as thin dual adapters; do not fork body |
| Agent domain guide | `agents-guide.md` file; `cli::AGENT_GUIDE` via `include_str!` | no (compile-time embed) | retain |
| Quality gate | `.github/workflows/ci.yml`; `Makefile` `bullseye` | Makefile also requires a clean tree | keep Makefile as local pre-push; add `--locked` (ENT-007) |
| Audit report path | `docs/audit/fable-2026-07.md` (singular); this file under `docs/audits/` (plural, skill contract) | path convention split | leave Fable in place; new reports follow `docs/audits/` |

Deliberate duplication that should **not** be collapsed: YAML status
(bullseye) vs SQLite attempt state (crosshair); 64 KiB read cap vs 4 KiB
store cap.

## Healthy structure worth retaining

- **Fable F1–F3 closure:** `process_group(0)`, `AttemptProcessGroup`
  SIGKILL on drop/timeout/error, `PIPE_DRAIN_TIMEOUT` on every arm
  (`src/runner.rs:128-276`). Tests:
  `timeout_kills_pipe_holding_grandchild_within_bound`,
  `backgrounded_command_does_not_wedge_pipe_drain`,
  `a_backgrounded_strategy_does_not_block_the_next_target`.
- **Fable F4 closure:** WAL + 5s busy timeout (`src/store.rs:220-223`);
  persist-failure fallback map (`src/store.rs:181-196, 390-412`);
  `status_polling_cannot_turn_a_failed_persist_into_a_backoff_free_rerun`.
- **Path identity:** `canonicalize` + `(yaml_path, target_id)` primary
  key; documented rename caveat in the agent guide.
- **SQL:** parameterized queries, explicit columns, no string-built
  identifiers.
- **Forward-compat YAML:** unknown fields ignored, required strategy
  fields still mandatory.
- **`--help-agent` embed:** `include_str!("../agents-guide.md")` cannot
  drift from the file.
- **CI skeleton:** stable toolchain, `Swatinem/rust-cache@v2`,
  `CARGO_INCREMENTAL=0`, `RUSTFLAGS=-D warnings`, fmt + clippy + test
  on `pull_request` and `master` (T4).
- **License / crate metadata:** Apache-2.0, repository URL, edition
  2024.
- **Tick independence (partial):** `MissedTickBehavior::Delay` avoids
  stampeding a catch-up queue; keep that when fixing ENT-001 catch-up
  *inside* `needs_attempt`, not by firing burst ticks.

## Hygiene posture

`hygiene.yaml` is **absent**. Hygiene posture is **not declared**.

Command:

```
/Users/marcelo/.claude/skills/hygiene/hygiene_check.py
```

Exit 1, validator exception:

```
FileNotFoundError: [Errno 2] No such file or directory:
'/Users/marcelo/work/github.com/marcelocantos/crosshair/hygiene.yaml'
```

No per-dimension held tiers, floors, or drift vector exist to validate.
This audit did not initialize `hygiene.yaml`.

**Overlap:** ENT-007 (CI `--locked`), ENT-008 (unused crates), ENT-004
(changelog/version), ENT-009 (gitignore), and ENT-012 (binary journey)
are the natural first `hygiene.yaml` items if the owner onboards later.
Do not treat this paragraph as a declared floor.

## Oracle coverage and residue

| Property | Decided by | Notes |
|---|---|---|
| fmt | shipped CI `cargo fmt --check` | green here |
| clippy `-D warnings` | shipped CI | green; unused crates invisible |
| unit/integration of runner/store/tick | shipped `cargo test` (22) | in-crate, real `sh` |
| lockfile = build input on PR | **nothing** | release-only `--locked` (ENT-007) |
| cron catch-up / tick phase | **nothing** | tests inject `now` (ENT-001) |
| timeout parse failures | **nothing** | silent default (ENT-002) |
| `retry` honored | **nothing** (accepted unread) | ENT-003 |
| status UTF-8 truncate | **nothing** | ENT-005 |
| multi-config load isolation | **nothing** | ENT-006 |
| unused direct crates | **nothing** | ENT-008 |
| binary CLI (`main`, clap, `--help-agent`) | **nothing** | ENT-012 |
| launchd KeepAlive | bullseye T1 attestation (manual, 2026-05-10) | not CI |
| advisory vulnerabilities / SBOM | **nothing** | cargo-deny/audit absent |
| dependabot / CODEOWNERS | **nothing** | files absent |
| hygiene floors | **undeclared** | |
| bullseye schema parity | **nothing in this repo** | out of scope |

Failed/skipped checks: hygiene validator (no file); cargo-machete /
cargo-deny / cargo-audit / nextest (not installed, not added);
bullseye tree (forbidden); live launchd.

**Owner residue (intent, not mechanical follow-up):**

1. Should missed `cron:` minutes catch up, or stay classic-cron skip?
   The 30m launchd sample is wrong under either answer until the
   sampler matches the policy.
2. Is `strategy.retry` a promise to implement, or config that should
   be stripped?
3. Accept Ubuntu-only CI for a macOS-first launchd product, or add a
   `macos-latest` job / periodic live KeepAlive check?
4. Onboard `hygiene.yaml`, or leave posture undeclared?

## Remediation sequence

1. **Scheduler (ENT-001).** Implement due-since-last-attempt for `cron:`
   (or, as a stop-gap, cap `--tick` at 1m when cron strategies are
   loaded and fix README/launchd to `30s`). Add a fake-clock test that
   would have failed today. Do this before any new cron consumer.
2. **Fail loud on bad timeout (ENT-002)** in the same load/run path;
   cheap and prevents false confidence in the hang ceiling.
3. **Resolve `retry` (ENT-003)** — implement or reject — so YAML stops
   lying. Then add `--locked` to CI (ENT-007).
4. **Docs/version (ENT-004)** on the same release-hygiene pass as (3).
5. **Unify `truncate` (ENT-005)**; isolate per-file YAML errors
   (ENT-006).
6. **Remove dead crates (ENT-008)**; ignore `.claudia-mcp-home/`
   (ENT-009); set SQLite `user_version` (ENT-011) before the next
   column is needed.
7. **Binary `--once` test (ENT-012)**; freeze a bullseye YAML fixture
   for `schema_version` (ENT-010).
8. If requested: declare `hygiene.yaml` from the gates that already
   exist (fmt, clippy, test, LICENSE, README) plus `--locked`, and
   park launchd/advisory scans as `planned`/`skipped` with `absent:`
   evidence. Re-run this audit against the same finding IDs.

Do not rewrite the crate into a workspace or split modules for the
runner↔store cycle; it is not the change-amplification problem.
