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
#[allow(clippy::large_enum_variant)] // CLI command enum; boxing args hurts ergonomics for a one-shot parse
enum Command {
    /// Generate identity, init config + SQLite store.
    Init(cli::InitArgs),

    /// Run the HTTP listener (peer protocol + local API + UI).
    Serve(cli::ServeArgs),

    /// Show the canonical instance id.
    #[command(name = "keys")]
    Keys(cli::KeysArgs),

    /// Sign a message and POST it to a remote peer's /n3ur0n/v0/messages.
    Send(cli::SendArgs),

    /// Inspect / refresh / cascade-discover the local peer directory.
    Peers(cli::PeersArgs),
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
        Command::Send(args) => cli::send(args).await,
        Command::Peers(args) => cli::peers(args).await,
    }
}
