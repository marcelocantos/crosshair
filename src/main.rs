// Copyright 2026 Marcelo Cantos
// SPDX-License-Identifier: Apache-2.0

use anyhow::Result;
use clap::Parser;
use crosshair::cli::{Cli, Command};
use crosshair::{status, tick};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    match cli.command {
        Command::Run(args) => tick::run(args).await,
        Command::Status(args) => status::run(args).await,
    }
}
