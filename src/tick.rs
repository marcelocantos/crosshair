// Copyright 2026 Marcelo Cantos
// SPDX-License-Identifier: Apache-2.0
//
// Drive the convergence loop. Each tick:
//   1. Reload all configured bullseye.yaml files (so target edits
//      take effect without restart).
//   2. For every strategy-bearing target whose trigger is due and whose
//      cooldown has expired, run the strategy.
//   3. Persist the outcome and compute the next cooldown from the
//      consecutive-failure count.
//
// Backoff schedule on failure (matches the bullseye 🎯T15 design):
//   1 → 30m, 2 → 2h, 3 → 6h, 4+ → 24h. Successful attempts clear
//   the cooldown so the next tick re-evaluates immediately.

use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use tokio::time;
use tracing::{info, warn};

use crate::cli::RunArgs;
use crate::loader::{StrategyTarget, load_strategy_targets};
use crate::runner::{AttemptOutcome, needs_attempt, run_attempt};
use crate::store::Store;

/// Backoff ladder indexed by the *new* consecutive-failure count
/// (i.e. after this attempt). Capped at the last entry.
const BACKOFF_LADDER: &[Duration] = &[
    Duration::from_secs(30 * 60),      // 1st failure: 30m
    Duration::from_secs(2 * 60 * 60),  // 2nd: 2h
    Duration::from_secs(6 * 60 * 60),  // 3rd: 6h
    Duration::from_secs(24 * 60 * 60), // 4th+: 24h
];

pub async fn run(args: RunArgs) -> Result<()> {
    let tick_interval = humantime::parse_duration(&args.tick)
        .with_context(|| format!("parse --tick {}", args.tick))?;
    let state_path = args.common.resolved_state_path()?;
    let store = Store::open(&state_path)?;

    info!(
        configs = ?args.common.configs,
        state = %state_path.display(),
        tick = %args.tick,
        "starting convergence loop"
    );

    if args.once {
        run_one_tick(&args, &store).await;
        return Ok(());
    }

    let mut ticker = time::interval(tick_interval);
    ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        run_one_tick(&args, &store).await;
    }
}

async fn run_one_tick(args: &RunArgs, store: &Store) {
    let targets = match load_strategy_targets(&args.common.configs) {
        Ok(ts) => ts,
        Err(e) => {
            warn!(error = %e, "failed to load configs; skipping tick");
            return;
        }
    };

    if targets.is_empty() {
        info!("no strategy-bearing targets to evaluate");
        return;
    }

    let now = Utc::now();
    for t in targets {
        if let Err(e) = converge_one(store, &t, now).await {
            warn!(target = %t.target_id, file = %t.yaml_path.display(), error = %e, "tick error");
        }
    }
}

async fn converge_one(store: &Store, t: &StrategyTarget, now: DateTime<Utc>) -> Result<()> {
    let prior = store.get_or_empty(t)?;
    if !needs_attempt(t, &prior, now)? {
        return Ok(());
    }
    if let Some(until) = prior.cooldown_until
        && until > now
    {
        return Ok(());
    }

    info!(target = %t.target_id, file = %t.yaml_path.display(), "running strategy");
    let outcome = run_attempt(t).await;
    let cooldown = next_cooldown(&prior, &outcome, now);

    if outcome.succeeded() {
        info!(target = %t.target_id, "strategy succeeded");
    } else {
        warn!(
            target = %t.target_id,
            exit = ?outcome.exit_code,
            timed_out = outcome.timed_out,
            "strategy failed"
        );
    }

    store.record_attempt(t, &outcome, cooldown, &prior)?;
    Ok(())
}

fn next_cooldown(
    prior: &crate::store::TargetState,
    outcome: &AttemptOutcome,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    if outcome.succeeded() {
        return None;
    }
    let new_failures = prior.consecutive_failures + 1;
    let idx = ((new_failures as usize).max(1) - 1).min(BACKOFF_LADDER.len() - 1);
    let dur = BACKOFF_LADDER[idx];
    let cd = chrono::Duration::from_std(dur).unwrap_or(chrono::Duration::hours(1));
    Some(now + cd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::CommonArgs;
    use crate::store::TargetState;
    use rusqlite::Connection;
    use std::fs;
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};
    use std::thread;
    use tempfile::{NamedTempFile, tempdir};

    fn make_outcome(success: bool) -> AttemptOutcome {
        AttemptOutcome {
            started_at: Utc::now(),
            finished_at: Utc::now(),
            exit_code: if success { Some(0) } else { Some(1) },
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        }
    }

    #[test]
    fn success_clears_cooldown() {
        let prior = TargetState::empty("/x".into(), "T1".into());
        let cd = next_cooldown(&prior, &make_outcome(true), Utc::now());
        assert!(cd.is_none());
    }

    #[test]
    fn failure_uses_backoff_ladder() {
        let now = Utc::now();
        for (failures_before, expected) in [
            (0u32, Duration::from_secs(30 * 60)),
            (1, Duration::from_secs(2 * 60 * 60)),
            (2, Duration::from_secs(6 * 60 * 60)),
            (3, Duration::from_secs(24 * 60 * 60)),
            (10, Duration::from_secs(24 * 60 * 60)),
        ] {
            let mut prior = TargetState::empty("/x".into(), "T1".into());
            prior.consecutive_failures = failures_before;
            let cd = next_cooldown(&prior, &make_outcome(false), now).unwrap();
            let delta = (cd - now).to_std().unwrap();
            assert!(
                delta.as_secs().abs_diff(expected.as_secs()) < 2,
                "failures_before={failures_before} expected={expected:?} got={delta:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_backgrounded_strategy_does_not_block_the_next_target() {
        let mut config = NamedTempFile::new().unwrap();
        config
            .write_all(
                br#"
schema_version: 3
targets:
  T1:
    name: backgrounded
    status: identified
    strategy:
      command: "sleep 30 &"
      trigger: "every:1h"
      timeout: 250ms
  T2:
    name: follows backgrounded work
    status: identified
    strategy:
      command: "echo second-target"
      trigger: "every:1h"
"#,
            )
            .unwrap();
        let state_dir = tempdir().unwrap();
        let args = RunArgs {
            common: CommonArgs {
                configs: vec![config.path().to_path_buf()],
                state: Some(state_dir.path().join("state.db")),
            },
            tick: "1s".to_string(),
            once: true,
        };
        let store = Store::open(args.common.state.as_ref().unwrap()).unwrap();

        time::timeout(Duration::from_secs(1), run_one_tick(&args, &store))
            .await
            .expect("backgrounded strategy wedged the tick");

        let targets = load_strategy_targets(&args.common.configs).unwrap();
        let second = targets
            .iter()
            .find(|target| target.target_id == "T2")
            .unwrap();
        assert!(
            store
                .get_or_empty(second)
                .unwrap()
                .last_success_at
                .is_some()
        );
    }

    #[tokio::test]
    async fn status_polling_cannot_turn_a_failed_persist_into_a_backoff_free_rerun() {
        let temp = tempdir().unwrap();
        let state_path = temp.path().join("state.db");
        let marker = temp.path().join("attempts");
        let mut config = NamedTempFile::new().unwrap();
        writeln!(
            config,
            r#"
schema_version: 3
targets:
  T1:
    name: persist-failure regression
    status: identified
    strategy:
      command: "printf x >> {}; exit 1"
      trigger: manual
"#,
            marker.display()
        )
        .unwrap();
        let targets = load_strategy_targets(&[config.path().to_path_buf()]).unwrap();
        let target = targets.into_iter().next().unwrap();
        let store = Store::open(&state_path).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let poll_errors = Arc::new(AtomicUsize::new(0));
        let (ready_tx, ready_rx) = mpsc::sync_channel(0);
        let poller_stop = Arc::clone(&stop);
        let poller_errors = Arc::clone(&poll_errors);
        let poller_path = state_path.clone();
        let poller_target = target.clone();
        let poller = thread::spawn(move || {
            let status_store = Store::open(&poller_path).unwrap();
            ready_tx.send(()).unwrap();
            // This is the same read operation performed once per target by
            // `status::run`; repeat it to model an active status poller.
            while !poller_stop.load(Ordering::Relaxed) {
                if status_store.get_or_empty(&poller_target).is_err() {
                    poller_errors.fetch_add(1, Ordering::Relaxed);
                }
                thread::yield_now();
            }
        });
        ready_rx.recv().unwrap();

        // Force a transient writer collision while the status reader is live.
        // Production connections wait five seconds; zero keeps this regression
        // test deterministic and exercises the fallback state explicitly.
        store.set_busy_timeout_for_test(Duration::ZERO).unwrap();
        let blocker = Connection::open(&state_path).unwrap();
        blocker.execute_batch("BEGIN EXCLUSIVE").unwrap();
        assert!(converge_one(&store, &target, Utc::now()).await.is_err());
        blocker.execute_batch("ROLLBACK").unwrap();

        let first = store.get_or_empty(&target).unwrap();
        let first_cooldown = first.cooldown_until.unwrap();
        assert_eq!(first.consecutive_failures, 1);
        assert_eq!(fs::read_to_string(&marker).unwrap(), "x");

        // A later tick before the fallback cooldown expires must not launch
        // the failing command again, even though its first result was not
        // committed to SQLite.
        converge_one(&store, &target, Utc::now()).await.unwrap();
        assert_eq!(fs::read_to_string(&marker).unwrap(), "x");

        // Once due, the next failure advances both the counter and cooldown
        // monotonically while the status reader continues polling.
        converge_one(&store, &target, first_cooldown).await.unwrap();
        let second = store.get_or_empty(&target).unwrap();

        stop.store(true, Ordering::Relaxed);
        poller.join().unwrap();
        assert_eq!(poll_errors.load(Ordering::Relaxed), 0);
        assert_eq!(second.consecutive_failures, 2);
        assert!(second.cooldown_until.unwrap() > first_cooldown);
        assert_eq!(fs::read_to_string(&marker).unwrap(), "xx");
    }
}
