// Copyright 2026 Marcelo Cantos
// SPDX-License-Identifier: Apache-2.0
//
// SQLite-backed per-target state. Keyed by (yaml_path, target_id)
// so the same target ID in different bullseye.yaml files stays
// distinct. Timestamps are stored as RFC3339 strings — readable in
// `sqlite3` shell, ordering-stable, and round-trips through chrono
// without per-OS unix-epoch drift.

use std::path::Path;

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
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create state dir {}", parent.display()))?;
        }
        let conn =
            Connection::open(path).with_context(|| format!("open sqlite at {}", path.display()))?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
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
    ) -> Result<()> {
        let key = t.yaml_path.display().to_string();
        let prev = self.get(&key, &t.target_id)?;

        let last_success_at = if outcome.succeeded() {
            Some(outcome.finished_at)
        } else {
            prev.as_ref().and_then(|p| p.last_success_at)
        };

        let consecutive_failures: u32 = if outcome.succeeded() {
            0
        } else {
            prev.as_ref().map(|p| p.consecutive_failures).unwrap_or(0) + 1
        };

        self.conn.execute(
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
                key,
                t.target_id,
                outcome.finished_at.to_rfc3339(),
                last_success_at.map(|t| t.to_rfc3339()),
                consecutive_failures,
                cooldown_until.map(|t| t.to_rfc3339()),
                outcome.exit_code,
                truncate(&outcome.stdout, 4096),
                truncate(&outcome.stderr, 4096),
                outcome.timed_out as i64,
            ],
        )?;
        Ok(())
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
        s.record_attempt(&t, &outcome(false, Some(1)), None)
            .unwrap();
        s.record_attempt(&t, &outcome(false, Some(1)), None)
            .unwrap();
        let after_two = s.get_or_empty(&t).unwrap();
        assert_eq!(after_two.consecutive_failures, 2);

        s.record_attempt(&t, &outcome(true, None), None).unwrap();
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
        s.record_attempt(&a, &outcome(true, None), None).unwrap();
        s.record_attempt(&b, &outcome(true, None), None).unwrap();
        let rows = s.list().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].target_id, "T1");
        assert_eq!(rows[1].target_id, "T2");
    }
}
