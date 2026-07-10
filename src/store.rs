// Copyright 2026 Marcelo Cantos
// SPDX-License-Identifier: Apache-2.0
//
// SQLite-backed per-target state. Keyed by (yaml_path, target_id)
// so the same target ID in different bullseye.yaml files stays
// distinct. Timestamps are stored as RFC3339 strings — readable in
// `sqlite3` shell, ordering-stable, and round-trips through chrono
// without per-OS unix-epoch drift.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use crate::loader::StrategyTarget;
use crate::runner::AttemptOutcome;

#[derive(Debug, Clone)]
pub struct TargetState {
    pub yaml_path: String,
    pub target_id: String,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub consecutive_failures: u32,
    pub cooldown_until: Option<DateTime<Utc>>,
    pub last_exit_code: Option<i32>,
    pub last_stdout: Option<String>,
    pub last_stderr: Option<String>,
    pub last_timed_out: bool,
}

impl TargetState {
    pub fn empty(yaml_path: String, target_id: String) -> Self {
        Self {
            yaml_path,
            target_id,
            last_attempt_at: None,
            last_success_at: None,
            consecutive_failures: 0,
            cooldown_until: None,
            last_exit_code: None,
            last_stdout: None,
            last_stderr: None,
            last_timed_out: false,
        }
    }
}

pub struct Store {
    conn: Connection,
    /// States which could not be persisted because SQLite was temporarily
    /// unavailable. They preserve backoff for this daemon's later ticks.
    fallback: Mutex<HashMap<(String, String), TargetState>>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create state dir {}", parent.display()))?;
        }
        let conn =
            Connection::open(path).with_context(|| format!("open sqlite at {}", path.display()))?;
        configure_connection(&conn)?;
        let store = Self {
            conn,
            fallback: Mutex::new(HashMap::new()),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        configure_connection(&conn)?;
        let store = Self {
            conn,
            fallback: Mutex::new(HashMap::new()),
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS target_state (
                yaml_path             TEXT NOT NULL,
                target_id             TEXT NOT NULL,
                last_attempt_at       TEXT,
                last_success_at       TEXT,
                consecutive_failures  INTEGER NOT NULL DEFAULT 0,
                cooldown_until        TEXT,
                last_exit_code        INTEGER,
                last_stdout           TEXT,
                last_stderr           TEXT,
                last_timed_out        INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (yaml_path, target_id)
            );
            "#,
        )?;
        Ok(())
    }

    pub fn get(&self, yaml_path: &str, target_id: &str) -> Result<Option<TargetState>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT yaml_path, target_id, last_attempt_at, last_success_at,
                      consecutive_failures, cooldown_until,
                      last_exit_code, last_stdout, last_stderr, last_timed_out
               FROM target_state WHERE yaml_path = ?1 AND target_id = ?2"#,
        )?;
        let row = stmt
            .query_row(params![yaml_path, target_id], row_to_state)
            .optional()?;
        Ok(row)
    }

    pub fn get_or_empty(&self, t: &StrategyTarget) -> Result<TargetState> {
        let key = t.yaml_path.display().to_string();
        if let Some(state) = self
            .fallback
            .lock()
            .expect("fallback state lock poisoned")
            .get(&(key.clone(), t.target_id.clone()))
            .cloned()
        {
            return Ok(state);
        }
        match self.get(&key, &t.target_id)? {
            Some(s) => Ok(s),
            None => Ok(TargetState::empty(key, t.target_id.clone())),
        }
    }

    /// Record a single attempt outcome — bumps the failure counter
    /// (or resets it on success), updates timestamps, and stores
    /// captured stdout/stderr for the next `crosshair status`.
    pub fn record_attempt(
        &self,
        t: &StrategyTarget,
        outcome: &AttemptOutcome,
        cooldown_until: Option<DateTime<Utc>>,
        prior: &TargetState,
    ) -> Result<()> {
        let key = t.yaml_path.display().to_string();
        let state = state_after_attempt(t, outcome, cooldown_until, prior);

        let result = self.conn.execute(
            r#"INSERT INTO target_state (
                    yaml_path, target_id, last_attempt_at, last_success_at,
                    consecutive_failures, cooldown_until,
                    last_exit_code, last_stdout, last_stderr, last_timed_out
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(yaml_path, target_id) DO UPDATE SET
                    last_attempt_at = excluded.last_attempt_at,
                    last_success_at = excluded.last_success_at,
                    consecutive_failures = excluded.consecutive_failures,
                    cooldown_until = excluded.cooldown_until,
                    last_exit_code = excluded.last_exit_code,
                    last_stdout = excluded.last_stdout,
                    last_stderr = excluded.last_stderr,
                    last_timed_out = excluded.last_timed_out
                "#,
            params![
                state.yaml_path,
                state.target_id,
                state.last_attempt_at.map(|t| t.to_rfc3339()),
                state.last_success_at.map(|t| t.to_rfc3339()),
                state.consecutive_failures,
                state.cooldown_until.map(|t| t.to_rfc3339()),
                state.last_exit_code,
                state.last_stdout,
                state.last_stderr,
                state.last_timed_out as i64,
            ],
        );
        match result {
            Ok(_) => {
                self.fallback
                    .lock()
                    .expect("fallback state lock poisoned")
                    .remove(&(key, t.target_id.clone()));
                Ok(())
            }
            Err(error) => {
                self.fallback
                    .lock()
                    .expect("fallback state lock poisoned")
                    .insert((key, t.target_id.clone()), state);
                Err(error.into())
            }
        }
    }

    pub fn list(&self) -> Result<Vec<TargetState>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT yaml_path, target_id, last_attempt_at, last_success_at,
                      consecutive_failures, cooldown_until,
                      last_exit_code, last_stdout, last_stderr, last_timed_out
               FROM target_state
               ORDER BY yaml_path, target_id"#,
        )?;
        let rows = stmt
            .query_map([], row_to_state)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

fn configure_connection(conn: &Connection) -> Result<()> {
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    Ok(())
}

fn state_after_attempt(
    t: &StrategyTarget,
    outcome: &AttemptOutcome,
    cooldown_until: Option<DateTime<Utc>>,
    prior: &TargetState,
) -> TargetState {
    TargetState {
        yaml_path: t.yaml_path.display().to_string(),
        target_id: t.target_id.clone(),
        last_attempt_at: Some(outcome.finished_at),
        last_success_at: outcome
            .succeeded()
            .then_some(outcome.finished_at)
            .or(prior.last_success_at),
        consecutive_failures: if outcome.succeeded() {
            0
        } else {
            prior.consecutive_failures + 1
        },
        cooldown_until,
        last_exit_code: outcome.exit_code,
        last_stdout: Some(truncate(&outcome.stdout, 4096)),
        last_stderr: Some(truncate(&outcome.stderr, 4096)),
        last_timed_out: outcome.timed_out,
    }
}

fn row_to_state(row: &rusqlite::Row<'_>) -> rusqlite::Result<TargetState> {
    Ok(TargetState {
        yaml_path: row.get(0)?,
        target_id: row.get(1)?,
        last_attempt_at: parse_rfc3339(row.get::<_, Option<String>>(2)?),
        last_success_at: parse_rfc3339(row.get::<_, Option<String>>(3)?),
        consecutive_failures: row.get::<_, i64>(4)? as u32,
        cooldown_until: parse_rfc3339(row.get::<_, Option<String>>(5)?),
        last_exit_code: row.get(6)?,
        last_stdout: row.get(7)?,
        last_stderr: row.get(8)?,
        last_timed_out: row.get::<_, i64>(9)? != 0,
    })
}

fn parse_rfc3339(s: Option<String>) -> Option<DateTime<Utc>> {
    s.as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…[truncated]", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Status, Strategy, Target};
    use std::path::PathBuf;
    use tempfile::NamedTempFile;

    fn fake_target() -> StrategyTarget {
        let s = Strategy {
            command: "true".to_string(),
            trigger: "manual".to_string(),
            timeout: None,
            retry: None,
        };
        StrategyTarget {
            yaml_path: PathBuf::from("/tmp/x.yaml"),
            target_id: "T1".to_string(),
            target: Target {
                name: "x".to_string(),
                status: Status::Identified,
                strategy: Some(s.clone()),
            },
            strategy: s,
        }
    }

    fn outcome(success: bool, exit: Option<i32>) -> AttemptOutcome {
        AttemptOutcome {
            started_at: Utc::now(),
            finished_at: Utc::now(),
            exit_code: if success { Some(0) } else { exit },
            stdout: "out".to_string(),
            stderr: "err".to_string(),
            timed_out: false,
        }
    }

    #[test]
    fn fresh_target_is_unknown() {
        let s = Store::open_in_memory().unwrap();
        let t = fake_target();
        let st = s.get_or_empty(&t).unwrap();
        assert_eq!(st.consecutive_failures, 0);
        assert!(st.last_attempt_at.is_none());
        assert!(st.last_success_at.is_none());
    }

    #[test]
    fn success_resets_consecutive_failures() {
        let s = Store::open_in_memory().unwrap();
        let t = fake_target();
        let prior = s.get_or_empty(&t).unwrap();
        s.record_attempt(&t, &outcome(false, Some(1)), None, &prior)
            .unwrap();
        let prior = s.get_or_empty(&t).unwrap();
        s.record_attempt(&t, &outcome(false, Some(1)), None, &prior)
            .unwrap();
        let after_two = s.get_or_empty(&t).unwrap();
        assert_eq!(after_two.consecutive_failures, 2);

        let prior = s.get_or_empty(&t).unwrap();
        s.record_attempt(&t, &outcome(true, None), None, &prior)
            .unwrap();
        let after_ok = s.get_or_empty(&t).unwrap();
        assert_eq!(after_ok.consecutive_failures, 0);
        assert!(after_ok.last_success_at.is_some());
    }

    #[test]
    fn list_returns_all_rows_sorted() {
        let s = Store::open_in_memory().unwrap();
        let mut a = fake_target();
        a.target_id = "T2".into();
        let mut b = fake_target();
        b.target_id = "T1".into();
        let prior_a = s.get_or_empty(&a).unwrap();
        s.record_attempt(&a, &outcome(true, None), None, &prior_a)
            .unwrap();
        let prior_b = s.get_or_empty(&b).unwrap();
        s.record_attempt(&b, &outcome(true, None), None, &prior_b)
            .unwrap();
        let rows = s.list().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].target_id, "T1");
        assert_eq!(rows[1].target_id, "T2");
    }

    #[test]
    fn file_store_enables_wal_and_busy_timeout() {
        let db = NamedTempFile::new().unwrap();
        let store = Store::open(db.path()).unwrap();
        let journal_mode: String = store
            .conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        let busy_timeout: i64 = store
            .conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
        assert!(busy_timeout > 0);
    }

    #[test]
    fn failed_persist_keeps_a_cooldown_for_the_next_tick() {
        let db = NamedTempFile::new().unwrap();
        let store = Store::open(db.path()).unwrap();
        // Make the test deterministic without waiting for the production
        // five-second busy timeout.
        store.conn.busy_timeout(Duration::ZERO).unwrap();
        let blocker = Connection::open(db.path()).unwrap();
        blocker.execute_batch("BEGIN EXCLUSIVE").unwrap();

        let target = fake_target();
        let prior = store.get_or_empty(&target).unwrap();
        let cooldown = Some(Utc::now() + chrono::Duration::minutes(30));
        assert!(
            store
                .record_attempt(&target, &outcome(false, Some(1)), cooldown, &prior)
                .is_err()
        );

        let fallback = store.get_or_empty(&target).unwrap();
        assert_eq!(fallback.consecutive_failures, 1);
        assert_eq!(fallback.cooldown_until, cooldown);
        blocker.execute_batch("ROLLBACK").unwrap();
    }
}
