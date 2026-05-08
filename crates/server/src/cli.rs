//! CLI subcommands.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use n3ur0n_node::IdentityFile;
use n3ur0n_server::{bootstrap, http};

const DEFAULT_PORT: u16 = 4242;

#[derive(Debug, Args)]
pub(crate) struct InitArgs {
    /// Override config directory (default: $XDG_CONFIG_HOME/n3ur0n or ~/.config/n3ur0n).
    #[arg(long)]
    pub(crate) config_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct ServeArgs {
    #[arg(long)]
    pub(crate) config_dir: Option<PathBuf>,

    #[arg(long, default_value_t = DEFAULT_PORT)]
    pub(crate) port: u16,

    /// Public endpoint advertised in describe_self.
    #[arg(long)]
    pub(crate) endpoint: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct KeysArgs {
    #[arg(long)]
    pub(crate) config_dir: Option<PathBuf>,
}

pub(crate) async fn init(args: InitArgs) -> Result<()> {
    let dir = args.config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let kp = bootstrap::create_identity(&dir)?;
    println!("instance id: {}", kp.instance_id());
    println!("config dir : {}", dir.display());
    println!("keys       : {}", bootstrap::keys_path(&dir).display());
    println!("database   : {}", bootstrap::db_path(&dir).display());
    Ok(())
}

pub(crate) async fn serve(args: ServeArgs) -> Result<()> {
    let dir = args.config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let node = bootstrap::load_node(&dir, args.endpoint).await?;
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], args.port));
    tracing::info!(instance_id = %node.instance_id(), port = args.port, "starting n3ur0n server");
    http::serve(addr, node).await
}

pub(crate) async fn keys(args: KeysArgs) -> Result<()> {
    let dir = args.config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let kp = IdentityFile::load(&bootstrap::keys_path(&dir))
        .with_context(|| format!("reading identity from {}", bootstrap::keys_path(&dir).display()))?;
    println!("{}", kp.instance_id());
    Ok(())
}
