// Copyright 2026 Marcelo Cantos
// SPDX-License-Identifier: Apache-2.0

use anyhow::Result;
use clap::Parser;
use crosshair::cli::{Cli, Command};
use crosshair::{status, tick};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.help_agent {
        print!("{}", Cli::help_agent_text());
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    match cli.command {
        Some(Command::Run(args)) => tick::run(args).await,
        Some(Command::Status(args)) => status::run(args).await,
        None => {
            use clap::CommandFactory;
            Cli::command().print_help()?;
            println!();
            Ok(())
        }
    }
}
