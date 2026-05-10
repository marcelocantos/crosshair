// Copyright 2026 Marcelo Cantos
// SPDX-License-Identifier: Apache-2.0

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Embedded agent guide. Surfaced via `crosshair --help-agent` so a coding
/// agent can pull both the CLI reference and the domain guide in one call.
pub const AGENT_GUIDE: &str = include_str!("../agents-guide.md");

#[derive(Debug, Parser)]
#[command(
    name = "crosshair",
    version,
    about = "Convergence executor daemon for bullseye targets"
)]
pub struct Cli {
    /// Print the full agent guide (CLI reference plus embedded
    /// agents-guide.md) and exit. Useful for coding agents that need
    /// the project's domain context alongside the CLI surface.
    #[arg(long, global = false, exclusive = true)]
    pub help_agent: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the convergence loop.
    Run(RunArgs),
    /// Show per-target executor state.
    Status(StatusArgs),
}

impl Cli {
    /// Render the `--help` text plus the embedded agent guide. Called
    /// from `main` when `--help-agent` is passed.
    pub fn help_agent_text() -> String {
        use clap::CommandFactory;
        let mut buf = Self::command().render_help().to_string();
        buf.push('\n');
        buf.push_str(AGENT_GUIDE);
        buf
    }
}

#[derive(Debug, Args, Clone)]
pub struct CommonArgs {
    /// Bullseye YAML files to scan. May be repeated.
    #[arg(short = 'c', long = "config", required = true, value_name = "PATH")]
    pub configs: Vec<PathBuf>,

    /// Path to the SQLite state file.
    /// Defaults to $HOME/.local/state/crosshair/state.db.
    #[arg(long, value_name = "PATH")]
    pub state: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    #[command(flatten)]
    pub common: CommonArgs,

    /// Tick interval (e.g., "30s", "5m"). Default: 30s.
    #[arg(long, default_value = "30s")]
    pub tick: String,

    /// Run a single tick and exit. Useful for testing or for cron-driven setups.
    #[arg(long)]
    pub once: bool,
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    #[command(flatten)]
    pub common: CommonArgs,
}

impl CommonArgs {
    /// Resolve the SQLite state path, falling back to $HOME/.local/state/crosshair/state.db.
    pub fn resolved_state_path(&self) -> anyhow::Result<PathBuf> {
        if let Some(p) = &self.state {
            return Ok(p.clone());
        }
        let home = std::env::var_os("HOME")
            .ok_or_else(|| anyhow::anyhow!("HOME is unset; pass --state explicitly"))?;
        Ok(PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("crosshair")
            .join("state.db"))
    }
}
