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

use std::process::Stdio;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

use crate::loader::StrategyTarget;

/// Default per-attempt timeout when the strategy doesn't specify one.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

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

/// Decide whether a strategy needs to run at all. Initial implementation:
/// terminal status (achieved or set_aside) means no work; anything else
/// means run the command and let it decide.
pub fn needs_attempt(t: &StrategyTarget) -> bool {
    !t.target.status.is_terminal()
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

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let stdout_task = tokio::spawn(read_to_string_capped(stdout, 64 * 1024));
    let stderr_task = tokio::spawn(read_to_string_capped(stderr, 64 * 1024));

    let wait = child.wait();
    match timeout(limit, wait).await {
        Ok(Ok(status)) => {
            let stdout = stdout_task.await.unwrap_or_default();
            let stderr = stderr_task.await.unwrap_or_default();
            AttemptOutcome {
                started_at,
                finished_at: Utc::now(),
                exit_code: status.code(),
                stdout,
                stderr,
                timed_out: false,
            }
        }
        Ok(Err(e)) => {
            let _ = child.kill().await;
            let stdout = stdout_task.await.unwrap_or_default();
            let stderr = stderr_task.await.unwrap_or_default();
            AttemptOutcome {
                started_at,
                finished_at: Utc::now(),
                exit_code: None,
                stdout,
                stderr: format!("{stderr}\nwait failed: {e}"),
                timed_out: false,
            }
        }
        Err(_) => {
            // Timeout — kill the child, drain pipes, surface as failure.
            let _ = child.kill().await;
            let stdout = stdout_task.await.unwrap_or_default();
            let stderr = stderr_task.await.unwrap_or_default();
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
    use std::path::PathBuf;

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

    #[test]
    fn terminal_status_skips_attempt() {
        let mut t = make_target("true", None);
        t.target.status = Status::Achieved;
        assert!(!needs_attempt(&t));
        t.target.status = Status::SetAside;
        assert!(!needs_attempt(&t));
        t.target.status = Status::Identified;
        assert!(needs_attempt(&t));
        t.target.status = Status::Converging;
        assert!(needs_attempt(&t));
    }
}
