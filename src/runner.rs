// Copyright 2026 Marcelo Cantos
// SPDX-License-Identifier: Apache-2.0
//
// Execute a target's strategy command, bound by a per-attempt
// timeout. Captures exit status, stdout, stderr. The satisfaction
// check is intentionally trivial in v0.1: trust the YAML status
// field — terminal-status targets are skipped, everything else is
// executed. The strategy command itself is responsible for being a
// no-op when the desired state is already in place (yadm-auto-sync
// is the canonical example: it logs a heartbeat when there are no
// changes to commit).

use std::collections::HashMap;
use std::process::Stdio;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Timelike, Utc};
use croner::Cron;
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::loader::StrategyTarget;
use crate::store::TargetState;

/// Default per-attempt timeout when the strategy doesn't specify one.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// The largest amount of strategy output retained before storing it.
const OUTPUT_CAP: usize = 64 * 1024;

/// Output readers must not keep a tick alive if a descendant retains a pipe.
const PIPE_DRAIN_TIMEOUT: Duration = Duration::from_millis(250);

/// Result of running a single attempt.
#[derive(Debug)]
pub struct AttemptOutcome {
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl AttemptOutcome {
    pub fn succeeded(&self) -> bool {
        !self.timed_out && self.exit_code == Some(0)
    }
}

/// Counts cron expressions compiled since the process started.
///
/// Compiling a cron expression is the only non-trivial work trigger
/// evaluation does, and the tick evaluates every strategy-bearing target
/// in the fleet on every interval. The counter exists so a benchmark can
/// assert how many compiles one pass costs, and so a change that starts
/// compiling per-target again fails the gate instead of quietly costing
/// the daemon a compile per target per minute. Take a delta around the
/// call under test; the absolute value means nothing.
static CRON_PARSES: AtomicU64 = AtomicU64::new(0);

/// Returns the process-wide number of cron expressions compiled so far.
pub fn cron_parse_count() -> u64 {
    CRON_PARSES.load(Ordering::Relaxed)
}

/// Compiled schedules, keyed by the expression they were compiled from.
///
/// A cron expression is a pure function of its text, so a compiled
/// schedule can be reused for the life of the process; the tick was
/// compiling one per strategy-bearing target per interval, which on a
/// fleet of a few hundred targets at the default 30-second tick is
/// around a million compiles a day for a fixed handful of distinct
/// expressions. The cache is keyed by the trimmed expression rather
/// than by target, so the fleet's many `*/5 * * * *` targets share one.
static CRON_CACHE: LazyLock<Mutex<HashMap<String, Arc<Cron>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Compile `expression`, or return the schedule already compiled from
/// identical text.
fn cron_schedule(expression: &str) -> Result<Arc<Cron>> {
    if let Some(schedule) = CRON_CACHE.lock().expect("cron cache").get(expression) {
        return Ok(Arc::clone(schedule));
    }
    CRON_PARSES.fetch_add(1, Ordering::Relaxed);
    let schedule = Arc::new(
        Cron::from_str(expression).with_context(|| format!("parse cron trigger {expression:?}"))?,
    );
    CRON_CACHE
        .lock()
        .expect("cron cache")
        .insert(expression.to_string(), Arc::clone(&schedule));
    Ok(schedule)
}

/// Decide whether a strategy is due for an automatic attempt.
///
/// Terminal targets and manual strategies never run. Cron strategies run once
/// in each matching minute; interval strategies run no more often than their
/// configured duration, measured from the previous attempt.
pub fn needs_attempt(t: &StrategyTarget, state: &TargetState, now: DateTime<Utc>) -> Result<bool> {
    if t.target.status.is_terminal() {
        return Ok(false);
    }

    let trigger = t.strategy.trigger.trim();
    if trigger == "manual" {
        return Ok(false);
    }

    if let Some(expression) = trigger.strip_prefix("cron:") {
        let expression = expression.trim();
        if expression.split_whitespace().count() != 5 {
            bail!("cron trigger must contain five fields: {expression:?}");
        }
        let schedule = cron_schedule(expression)?;
        let current_minute = now
            .with_second(0)
            .and_then(|time| time.with_nanosecond(0))
            .expect("zero is a valid second and nanosecond");
        if !schedule
            .is_time_matching(&current_minute)
            .with_context(|| format!("evaluate cron trigger {expression:?}"))?
        {
            return Ok(false);
        }

        return Ok(state
            .last_attempt_at
            .is_none_or(|previous| previous < current_minute));
    }

    if let Some(duration) = trigger.strip_prefix("every:") {
        let duration = humantime::parse_duration(duration.trim())
            .with_context(|| format!("parse interval trigger {trigger:?}"))?;
        let interval = chrono::Duration::from_std(duration)
            .with_context(|| format!("interval trigger is too large: {trigger:?}"))?;
        return Ok(state
            .last_attempt_at
            .is_none_or(|previous| now - previous >= interval));
    }

    bail!("unsupported strategy trigger {trigger:?}")
}

#[cfg(test)]
mod cron_cache_tests {
    use super::*;
    use chrono::TimeZone;

    /// Identical expressions share one compiled schedule; different ones
    /// do not. Asserted by pointer identity rather than by the compile
    /// counter, which is process-wide while cargo runs tests in
    /// parallel.
    #[test]
    fn identical_expressions_share_a_compiled_schedule() {
        let a = cron_schedule("*/7 * * * *").unwrap();
        let b = cron_schedule("*/7 * * * *").unwrap();
        assert!(Arc::ptr_eq(&a, &b), "second compile of the same text");

        let other = cron_schedule("*/11 * * * *").unwrap();
        assert!(!Arc::ptr_eq(&a, &other), "different text shared a schedule");
    }

    /// A cached schedule still answers about the time it is asked
    /// about, not the time it was compiled at.
    #[test]
    fn a_cached_schedule_still_matches_by_time() {
        let schedule = cron_schedule("*/5 * * * *").unwrap();
        let on = Utc.with_ymd_and_hms(2026, 9, 7, 10, 5, 0).unwrap();
        let off = Utc.with_ymd_and_hms(2026, 9, 7, 10, 6, 0).unwrap();
        assert!(schedule.is_time_matching(&on).unwrap());
        assert!(!schedule.is_time_matching(&off).unwrap());
    }
}

/// Resolve the strategy's per-attempt timeout, falling back to default.
pub fn resolved_timeout(t: &StrategyTarget) -> Duration {
    t.strategy
        .timeout
        .as_deref()
        .and_then(|s| humantime::parse_duration(s).ok())
        .unwrap_or(DEFAULT_TIMEOUT)
}

/// Run the strategy command with a hard ceiling. The command is
/// dispatched through `sh -c` so the YAML can carry shell idioms
/// (pipes, env interpolation, semicolons) without crosshair having
/// to lex them. This matches the existing yadm-auto-sync pattern.
pub async fn run_attempt(t: &StrategyTarget) -> AttemptOutcome {
    let started_at = Utc::now();
    let limit = resolved_timeout(t);

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(&t.strategy.command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .kill_on_drop(true);
    // A strategy and every process it starts must be killed together.
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return AttemptOutcome {
                started_at,
                finished_at: Utc::now(),
                exit_code: None,
                stdout: String::new(),
                stderr: format!("spawn failed: {e}"),
                timed_out: false,
            };
        }
    };
    let process_group = AttemptProcessGroup::new(child.id().expect("spawned child has a PID"));

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let mut stdout_task = tokio::spawn(read_to_string_capped(stdout, OUTPUT_CAP));
    let mut stderr_task = tokio::spawn(read_to_string_capped(stderr, OUTPUT_CAP));

    let wait = child.wait();
    match timeout(limit, wait).await {
        Ok(Ok(status)) => {
            // The shell may have exited while a background descendant remains.
            // It is still part of this attempt, so terminate it before draining.
            process_group.kill();
            let (stdout, mut stderr, drain_timed_out) =
                drain_pipes(&mut stdout_task, &mut stderr_task).await;
            if drain_timed_out {
                stderr.push_str("\ncrosshair: output drain timed out");
            }
            AttemptOutcome {
                started_at,
                finished_at: Utc::now(),
                exit_code: if drain_timed_out { None } else { status.code() },
                stdout,
                stderr,
                timed_out: drain_timed_out,
            }
        }
        Ok(Err(e)) => {
            process_group.kill();
            let _ = timeout(PIPE_DRAIN_TIMEOUT, child.wait()).await;
            let (stdout, mut stderr, drain_timed_out) =
                drain_pipes(&mut stdout_task, &mut stderr_task).await;
            if drain_timed_out {
                stderr.push_str("\ncrosshair: output drain timed out");
            }
            AttemptOutcome {
                started_at,
                finished_at: Utc::now(),
                exit_code: None,
                stdout,
                stderr: format!("{stderr}\nwait failed: {e}"),
                timed_out: drain_timed_out,
            }
        }
        Err(_) => {
            // Timeout — kill the process group, then drain only within a bound.
            process_group.kill();
            let _ = timeout(PIPE_DRAIN_TIMEOUT, child.wait()).await;
            let (stdout, mut stderr, drain_timed_out) =
                drain_pipes(&mut stdout_task, &mut stderr_task).await;
            if drain_timed_out {
                stderr.push_str("\ncrosshair: output drain timed out");
            }
            AttemptOutcome {
                started_at,
                finished_at: Utc::now(),
                exit_code: None,
                stdout,
                stderr: format!(
                    "{stderr}\ncrosshair: killed after {}",
                    humantime::format_duration(limit)
                ),
                timed_out: true,
            }
        }
    }
}

/// A process group is the lifetime boundary for an attempt. Dropping the
/// future (including task cancellation) therefore cannot leave descendants
/// behind merely because Tokio's `kill_on_drop` only knows the direct child.
struct AttemptProcessGroup {
    #[cfg(unix)]
    pgid: u32,
}

impl AttemptProcessGroup {
    fn new(pgid: u32) -> Self {
        #[cfg(unix)]
        {
            Self { pgid }
        }
        #[cfg(not(unix))]
        {
            let _ = pgid;
            Self {}
        }
    }

    fn kill(&self) {
        #[cfg(unix)]
        {
            // Negative PID targets the Unix process group. ESRCH is expected when
            // every member has already exited, and is harmless.
            let result = unsafe { libc::kill(-(self.pgid as libc::pid_t), libc::SIGKILL) };
            if result != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    tracing::warn!(pgid = self.pgid, error = %error, "failed to kill strategy process group");
                }
            }
        }
    }
}

impl Drop for AttemptProcessGroup {
    fn drop(&mut self) {
        self.kill();
    }
}

async fn drain_pipes(
    stdout_task: &mut JoinHandle<String>,
    stderr_task: &mut JoinHandle<String>,
) -> (String, String, bool) {
    match timeout(PIPE_DRAIN_TIMEOUT, async {
        let stdout = (&mut *stdout_task).await.unwrap_or_default();
        let stderr = (&mut *stderr_task).await.unwrap_or_default();
        (stdout, stderr)
    })
    .await
    {
        Ok((stdout, stderr)) => (stdout, stderr, false),
        Err(_) => {
            stdout_task.abort();
            stderr_task.abort();
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            (String::new(), String::new(), true)
        }
    }
}

async fn read_to_string_capped<R: tokio::io::AsyncRead + Unpin>(reader: R, cap: usize) -> String {
    let mut buf = Vec::with_capacity(cap.min(4096));
    let mut tmp = [0u8; 4096];
    let mut r = BufReader::new(reader);
    while buf.len() < cap {
        let want = (cap - buf.len()).min(tmp.len());
        let n = match r.read(&mut tmp[..want]).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        buf.extend_from_slice(&tmp[..n]);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::StrategyTarget;
    use crate::schema::{Status, Strategy, Target};
    use chrono::TimeZone;
    #[cfg(unix)]
    use std::fs;
    use std::path::PathBuf;
    #[cfg(unix)]
    use std::time::Instant;
    #[cfg(unix)]
    use tempfile::tempdir;

    fn make_target(command: &str, timeout: Option<&str>) -> StrategyTarget {
        let strategy = Strategy {
            command: command.to_string(),
            trigger: "manual".to_string(),
            timeout: timeout.map(str::to_string),
            retry: None,
        };
        StrategyTarget {
            yaml_path: PathBuf::from("/tmp/test.yaml"),
            target_id: "T1".to_string(),
            target: Target {
                name: "test".to_string(),
                status: Status::Identified,
                strategy: Some(strategy.clone()),
            },
            strategy,
        }
    }

    #[tokio::test]
    async fn captures_stdout_and_zero_exit() {
        let t = make_target("echo hello", None);
        let o = run_attempt(&t).await;
        assert!(o.succeeded());
        assert_eq!(o.exit_code, Some(0));
        assert!(o.stdout.contains("hello"));
        assert!(!o.timed_out);
    }

    #[tokio::test]
    async fn captures_nonzero_exit() {
        let t = make_target("exit 7", None);
        let o = run_attempt(&t).await;
        assert!(!o.succeeded());
        assert_eq!(o.exit_code, Some(7));
    }

    #[tokio::test]
    async fn enforces_per_attempt_timeout() {
        let t = make_target("sleep 5", Some("250ms"));
        let o = run_attempt(&t).await;
        assert!(o.timed_out);
        assert!(!o.succeeded());
        assert!(o.stderr.contains("killed after"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_pipe_holding_grandchild_within_bound() {
        let dir = tempdir().unwrap();
        let pid_file = dir.path().join("grandchild.pid");
        let command = format!(
            "sleep 30 & child=$!; echo $child > {}; wait",
            pid_file.display()
        );
        let t = make_target(&command, Some("250ms"));

        let started = Instant::now();
        let o = run_attempt(&t).await;
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(o.timed_out);

        let pid = fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        wait_for_process_exit(pid).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn backgrounded_command_does_not_wedge_pipe_drain() {
        let dir = tempdir().unwrap();
        let pid_file = dir.path().join("background.pid");
        let command = format!("sleep 30 & echo $! > {}", pid_file.display());
        let t = make_target(&command, Some("250ms"));

        let started = Instant::now();
        let o = run_attempt(&t).await;
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(o.succeeded());

        let pid = fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        wait_for_process_exit(pid).await;
    }

    #[cfg(unix)]
    async fn wait_for_process_exit(pid: libc::pid_t) {
        timeout(Duration::from_secs(1), async {
            while process_exists(pid) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("strategy descendant {pid} survived cleanup"));
    }

    #[cfg(unix)]
    fn process_exists(pid: libc::pid_t) -> bool {
        let result = unsafe { libc::kill(pid, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    #[test]
    fn terminal_status_skips_attempt() {
        let mut t = make_target("true", None);
        let state = TargetState::empty("/tmp/test.yaml".into(), "T1".into());
        t.target.status = Status::Achieved;
        assert!(!needs_attempt(&t, &state, Utc::now()).unwrap());
        t.target.status = Status::SetAside;
        assert!(!needs_attempt(&t, &state, Utc::now()).unwrap());
        t.target.status = Status::Identified;
        assert!(!needs_attempt(&t, &state, Utc::now()).unwrap());
        t.target.status = Status::Converging;
        assert!(!needs_attempt(&t, &state, Utc::now()).unwrap());
    }

    #[test]
    fn daily_cron_runs_once_per_day_across_ticks() {
        let mut t = make_target("true", None);
        t.strategy.trigger = "cron:0 0 * * *".into();
        let first_tick = Utc.with_ymd_and_hms(2026, 7, 10, 0, 0, 15).unwrap();
        let mut state = TargetState::empty("/tmp/test.yaml".into(), "T1".into());

        assert!(needs_attempt(&t, &state, first_tick).unwrap());
        state.last_attempt_at = Some(first_tick);
        assert!(!needs_attempt(&t, &state, first_tick + chrono::Duration::seconds(30)).unwrap());
        assert!(!needs_attempt(&t, &state, first_tick + chrono::Duration::hours(12)).unwrap());
        assert!(needs_attempt(&t, &state, first_tick + chrono::Duration::days(1)).unwrap());
    }

    #[test]
    fn interval_uses_last_attempt_as_its_rate_limit() {
        let mut t = make_target("true", None);
        t.strategy.trigger = "every:24h".into();
        let now = Utc.with_ymd_and_hms(2026, 7, 10, 12, 0, 0).unwrap();
        let mut state = TargetState::empty("/tmp/test.yaml".into(), "T1".into());

        assert!(needs_attempt(&t, &state, now).unwrap());
        state.last_attempt_at = Some(now);
        assert!(!needs_attempt(&t, &state, now + chrono::Duration::hours(23)).unwrap());
        assert!(needs_attempt(&t, &state, now + chrono::Duration::hours(24)).unwrap());
    }
}
