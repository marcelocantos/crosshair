// Copyright 2026 Marcelo Cantos
// SPDX-License-Identifier: Apache-2.0
//
// Load bullseye.yaml files from disk and expose every target that
// carries a `strategy` block. Each loaded file is canonicalised so
// the same path passed twice (or via different relative forms) keys
// to the same entry in the SQLite store.

use std::path::{Path, PathBuf};

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

/// Load every config file and return its strategy-bearing targets.
/// Files that fail to load surface the error to the caller, who
/// decides whether to skip or abort. We canonicalise paths up front
/// so duplicate `--config` flags collapse into one entry.
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
        let doc = load_doc(path)?;
        for (id, target) in doc.targets {
            let Some(strategy) = target.strategy.clone() else {
                continue;
            };
            out.push(StrategyTarget {
                yaml_path: path.clone(),
                target_id: id,
                target,
                strategy,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
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
