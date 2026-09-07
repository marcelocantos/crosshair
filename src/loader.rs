// Copyright 2026 Marcelo Cantos
// SPDX-License-Identifier: Apache-2.0
//
// Load bullseye.yaml files from disk and expose every target that
// carries a `strategy` block. Each loaded file is canonicalised so
// the same path passed twice (or via different relative forms) keys
// to the same entry in the SQLite store.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use anyhow::{Context, Result};

use crate::schema::{Doc, Strategy, Target};

/// A target that carries a strategy, paired with the file it came from.
#[derive(Debug, Clone)]
pub struct StrategyTarget {
    pub yaml_path: PathBuf,
    pub target_id: String,
    pub target: Target,
    pub strategy: Strategy,
}

impl StrategyTarget {
    pub fn key(&self) -> (String, &str) {
        (
            self.yaml_path.display().to_string(),
            self.target_id.as_str(),
        )
    }
}

/// Load a single bullseye.yaml file from disk.
pub fn load_doc(path: &Path) -> Result<Doc> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let doc: Doc =
        serde_yaml_ng::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    Ok(doc)
}

/// What a cached parse was of: a file's modification time and length.
///
/// A tick that finds both unchanged reuses the parse. This is the
/// standard staleness pair rather than a content hash because hashing
/// still reads the whole file, and reading it is most of what we are
/// trying to avoid. It is also what git's index uses to decide a file
/// is unchanged.
///
/// The pair can miss an edit that preserves both — a same-length write
/// inside one filesystem timestamp tick. That is a real hole, and the
/// consequence is bounded and self-correcting: crosshair runs the
/// previous version of that one target's strategy for as long as the
/// file stays untouched, and the next write of any size or second
/// picks it up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

impl Stamp {
    fn of(path: &Path) -> Result<Stamp> {
        let meta = std::fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
        Ok(Stamp {
            modified: meta.modified().ok(),
            len: meta.len(),
        })
    }
}

/// Parsed documents, keyed by canonical path, kept across ticks.
///
/// The daemon re-reads its whole fleet every tick — every 30 seconds by
/// default — so that a target edit takes effect without a restart. On a
/// two-dozen-file fleet that is 700 KB of YAML parsed 2,880 times a day
/// to discover, almost every time, that nothing changed.
type DocCache = HashMap<PathBuf, (Stamp, Arc<Doc>)>;
static DOC_CACHE: LazyLock<Mutex<DocCache>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Counts documents actually parsed since the process started, as
/// opposed to served from the cache. A benchmark asserts on it, and a
/// change that defeats the cache shows up here rather than as a slow
/// daemon nobody looks at. Take a delta; the absolute value means
/// nothing.
static DOC_PARSES: AtomicU64 = AtomicU64::new(0);

/// Returns the process-wide number of bullseye.yaml documents parsed.
pub fn doc_parse_count() -> u64 {
    DOC_PARSES.load(Ordering::Relaxed)
}

/// Load `path`, reusing the previous parse when the file's modification
/// time and length are both unchanged since it was taken.
pub(crate) fn load_doc_cached(path: &Path) -> Result<Arc<Doc>> {
    let stamp = Stamp::of(path)?;
    if let Some((cached, doc)) = DOC_CACHE.lock().expect("doc cache").get(path)
        && *cached == stamp
    {
        return Ok(Arc::clone(doc));
    }
    // Parsed outside the lock's scope above so a slow parse of one file
    // does not block a concurrent load of another.
    DOC_PARSES.fetch_add(1, Ordering::Relaxed);
    let doc = Arc::new(load_doc(path)?);
    DOC_CACHE
        .lock()
        .expect("doc cache")
        .insert(path.to_path_buf(), (stamp, Arc::clone(&doc)));
    Ok(doc)
}

/// Load every config file and return its strategy-bearing targets.
/// Files that fail to load surface the error to the caller, who
/// decides whether to skip or abort. We canonicalise paths up front
/// so duplicate `--config` flags collapse into one entry.
///
/// A file whose modification time and length are unchanged since the
/// last call is not re-read or re-parsed (🎯T9); its targets are cloned
/// out of the cached document instead.
pub fn load_strategy_targets(paths: &[PathBuf]) -> Result<Vec<StrategyTarget>> {
    let mut canonical: Vec<PathBuf> = Vec::with_capacity(paths.len());
    for p in paths {
        let abs = p
            .canonicalize()
            .with_context(|| format!("canonicalise {}", p.display()))?;
        if !canonical.contains(&abs) {
            canonical.push(abs);
        }
    }

    let mut out = Vec::new();
    for path in &canonical {
        let doc = load_doc_cached(path)?;
        for (id, target) in &doc.targets {
            let Some(strategy) = target.strategy.clone() else {
                continue;
            };
            out.push(StrategyTarget {
                yaml_path: path.clone(),
                target_id: id.clone(),
                target: target.clone(),
                strategy,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};
    use tempfile::NamedTempFile;

    fn yaml_file(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    #[test]
    fn skips_targets_without_strategy() {
        let f = yaml_file(
            r#"
schema_version: 3
targets:
  T1:
    name: no strategy
    status: identified
  T2:
    name: with strategy
    status: identified
    strategy:
      command: "true"
      trigger: "manual"
"#,
        );
        let targets = load_strategy_targets(&[f.path().to_path_buf()]).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].target_id, "T2");
        assert_eq!(targets[0].strategy.command, "true");
    }

    /// An unchanged file is not re-read. Asserted by pointer identity
    /// rather than by the parse counter, because the counter is
    /// process-wide and cargo runs these tests in parallel.
    #[test]
    fn unchanged_file_is_served_from_the_cache() {
        let f = yaml_file(
            r#"
schema_version: 3
targets:
  T1:
    name: one
    status: identified
"#,
        );
        let first = load_doc_cached(f.path()).unwrap();
        let second = load_doc_cached(f.path()).unwrap();
        assert!(
            Arc::ptr_eq(&first, &second),
            "second load re-parsed an unchanged file"
        );
    }

    /// A file whose content and length changed is re-read, and the new
    /// targets are the ones returned. This is the property the cache
    /// must not break: crosshair exists to pick up target edits without
    /// a restart.
    #[test]
    fn edited_file_is_reloaded() {
        let mut f = yaml_file(
            r#"
schema_version: 3
targets:
  T1:
    name: one
    status: identified
    strategy:
      command: "true"
      trigger: "manual"
"#,
        );
        let before = load_strategy_targets(&[f.path().to_path_buf()]).unwrap();
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].strategy.command, "true");

        // A longer command, so both halves of the staleness pair move.
        f.as_file_mut().set_len(0).unwrap();
        f.as_file_mut().seek(SeekFrom::Start(0)).unwrap();
        f.write_all(
            br#"
schema_version: 3
targets:
  T1:
    name: one
    status: identified
    strategy:
      command: "make bullseye && echo done"
      trigger: "manual"
"#,
        )
        .unwrap();
        f.flush().unwrap();

        let after = load_strategy_targets(&[f.path().to_path_buf()]).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].strategy.command, "make bullseye && echo done");
    }

    #[test]
    fn deduplicates_repeated_paths() {
        let f = yaml_file(
            r#"
schema_version: 3
targets:
  T1:
    name: one
    status: identified
    strategy:
      command: "true"
      trigger: "manual"
"#,
        );
        let p = f.path().to_path_buf();
        let targets = load_strategy_targets(&[p.clone(), p]).unwrap();
        assert_eq!(targets.len(), 1);
    }
}
