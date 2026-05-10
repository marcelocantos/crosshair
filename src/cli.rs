// Copyright 2026 Marcelo Cantos
// SPDX-License-Identifier: Apache-2.0

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "crosshair",
    version,
    about = "Convergence executor daemon for bullseye targets"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the convergence loop.
    Run(RunArgs),
    /// Show per-target executor state.
    Status(StatusArgs),
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
