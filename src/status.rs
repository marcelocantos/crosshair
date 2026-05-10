// Copyright 2026 Marcelo Cantos
// SPDX-License-Identifier: Apache-2.0
//
// `crosshair status` — print one row per strategy-bearing target.
// Joins the live YAML view (so we list targets that have never run
// yet) with the persisted state from SQLite.

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::cli::StatusArgs;
use crate::loader::load_strategy_targets;
use crate::store::{Store, TargetState};

pub async fn run(args: StatusArgs) -> Result<()> {
    let state_path = args.common.resolved_state_path()?;
    let store = Store::open(&state_path)?;
    let targets = load_strategy_targets(&args.common.configs)?;

    if targets.is_empty() {
        println!("(no strategy-bearing targets configured)");
        return Ok(());
    }

    let now = Utc::now();
    println!(
        "{:<24} {:<8} {:<10} {:<19} {:<19} {:<8} COOLDOWN",
        "TARGET", "STATUS", "OUTCOME", "LAST ATTEMPT", "LAST SUCCESS", "FAILS"
    );
    for t in &targets {
        let state = store.get_or_empty(t)?;
        let outcome = outcome_label(&state);
        let cooldown = cooldown_label(&state, now);
        println!(
            "{:<24} {:<8} {:<10} {:<19} {:<19} {:<8} {}",
            truncate(&t.target_id, 24),
            status_label(t.target.status),
            outcome,
            ts(&state.last_attempt_at),
            ts(&state.last_success_at),
            state.consecutive_failures,
            cooldown,
        );
    }
    Ok(())
}

fn status_label(s: crate::schema::Status) -> &'static str {
    match s {
        crate::schema::Status::Identified => "ident",
        crate::schema::Status::Converging => "conv",
        crate::schema::Status::Achieved => "done",
        crate::schema::Status::SetAside => "aside",
    }
}

fn outcome_label(state: &TargetState) -> &'static str {
    if state.last_attempt_at.is_none() {
        "—"
    } else if state.last_timed_out {
        "timeout"
    } else if state.last_exit_code == Some(0) {
        "ok"
    } else {
        "fail"
    }
}

fn cooldown_label(state: &TargetState, now: DateTime<Utc>) -> String {
    match state.cooldown_until {
        None => "—".to_string(),
        Some(until) if until <= now => "expired".to_string(),
        Some(until) => {
            let remaining = (until - now).to_std().unwrap_or_default();
            format!(
                "in {}",
                humantime::format_duration(round_to_seconds(remaining))
            )
        }
    }
}

fn round_to_seconds(d: std::time::Duration) -> std::time::Duration {
    std::time::Duration::from_secs(d.as_secs())
}

fn ts(t: &Option<DateTime<Utc>>) -> String {
    match t {
        Some(t) => t.format("%Y-%m-%d %H:%M:%S").to_string(),
        None => "—".to_string(),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max - 1])
    }
}
