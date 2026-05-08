//! N3UR0N publisher binary entry point.

use anyhow::Result;
use clap::{Parser, Subcommand};

mod cli;

#[derive(Debug, Parser)]
#[command(name = "n3ur0n", version, about = "N3UR0N publisher node")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Generate identity, init config + SQLite store.
    Init(cli::InitArgs),

    /// Run the HTTP listener (peer protocol + local API + UI).
    Serve(cli::ServeArgs),

    /// Show the canonical instance id.
    #[command(name = "keys")]
    Keys(cli::KeysArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Init(args) => cli::init(args).await,
        Command::Serve(args) => cli::serve(args).await,
        Command::Keys(args) => cli::keys(args).await,
    }
}
