// Copyright 2026 Marcelo Cantos
// SPDX-License-Identifier: Apache-2.0
//
// Subset of the bullseye YAML schema that crosshair reads. Mirrors
// the field names and serde representations used by bullseye 0.25+,
// but only the fields crosshair cares about — anything else is
// ignored on load, so this code keeps loading newer files it doesn't
// fully understand.

use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Doc {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub targets: BTreeMap<String, Target>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Target {
    pub name: String,
    #[serde(default)]
    pub status: Status,
    #[serde(default)]
    pub strategy: Option<Strategy>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Identified,
    Converging,
    Achieved,
    SetAside,
}

impl Status {
    pub fn is_terminal(self) -> bool {
        matches!(self, Status::Achieved | Status::SetAside)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Strategy {
    pub command: String,
    pub trigger: String,
    #[serde(default)]
    pub timeout: Option<String>,
    #[serde(default)]
    pub retry: Option<RetryPolicy>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RetryPolicy {
    #[serde(default)]
    pub max_attempts: Option<u32>,
    #[serde(default)]
    pub backoff: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_doc() {
        let yaml = r#"
schema_version: 3
targets:
  T1:
    name: example
    status: identified
    strategy:
      command: "echo hi"
      trigger: "manual"
"#;
        let doc: Doc = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(doc.schema_version, 3);
        let t = doc.targets.get("T1").unwrap();
        assert_eq!(t.name, "example");
        assert_eq!(t.status, Status::Identified);
        let s = t.strategy.as_ref().unwrap();
        assert_eq!(s.command, "echo hi");
        assert_eq!(s.trigger, "manual");
    }

    #[test]
    fn ignores_unknown_fields() {
        let yaml = r#"
schema_version: 3
unknown_top_level: foo
targets:
  T1:
    name: example
    status: converging
    value: 5
    cost: 3
    acceptance:
      - thing 1
    showcase: true
    strategy:
      command: "true"
      trigger: "cron:0 * * * *"
      timeout: "30m"
      retry:
        max_attempts: 5
        backoff: "exponential"
"#;
        let doc: Doc = serde_yaml_ng::from_str(yaml).unwrap();
        let t = doc.targets.get("T1").unwrap();
        assert_eq!(t.status, Status::Converging);
        let s = t.strategy.as_ref().unwrap();
        assert_eq!(s.timeout.as_deref(), Some("30m"));
        let r = s.retry.as_ref().unwrap();
        assert_eq!(r.max_attempts, Some(5));
    }

    #[test]
    fn target_without_strategy_loads() {
        let yaml = r#"
schema_version: 3
targets:
  T1:
    name: example
    status: identified
"#;
        let doc: Doc = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(doc.targets.get("T1").unwrap().strategy.is_none());
    }
}
