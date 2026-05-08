//! CLI subcommands.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use n3ur0n_core::message::ProtocolVerb;
use n3ur0n_node::IdentityFile;
use n3ur0n_server::{bootstrap, client, http};
use serde_json::Value;

const DEFAULT_PORT: u16 = 4242;

#[derive(Debug, Args)]
pub(crate) struct InitArgs {
    /// Override config directory (default: $XDG_CONFIG_HOME/n3ur0n or ~/.config/n3ur0n).
    #[arg(long, env = "N3UR0N_CONFIG_DIR")]
    pub(crate) config_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct ServeArgs {
    #[arg(long, env = "N3UR0N_CONFIG_DIR")]
    pub(crate) config_dir: Option<PathBuf>,

    #[arg(long, default_value_t = DEFAULT_PORT)]
    pub(crate) port: u16,

    /// Public endpoint advertised in describe_self.
    #[arg(long)]
    pub(crate) endpoint: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct KeysArgs {
    #[arg(long, env = "N3UR0N_CONFIG_DIR")]
    pub(crate) config_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct SendArgs {
    /// Local identity to sign with.
    #[arg(long, env = "N3UR0N_CONFIG_DIR")]
    pub(crate) config_dir: Option<PathBuf>,

    /// Base URL of the remote peer (e.g. http://node-b:4242).
    #[arg(long)]
    pub(crate) endpoint: String,

    /// Verb to send. One of: ping, describe_self, get_known_peers, invoke.
    #[arg(long, default_value = "ping")]
    pub(crate) verb: String,

    /// JSON payload (defaults to `{}`).
    #[arg(long, default_value = "{}")]
    pub(crate) payload: String,
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

pub(crate) async fn send(args: SendArgs) -> Result<()> {
    let dir = args.config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let kp = IdentityFile::load(&bootstrap::keys_path(&dir))
        .with_context(|| format!("loading identity from {}", bootstrap::keys_path(&dir).display()))?;
    let verb = parse_verb(&args.verb)?;
    let payload: Value = serde_json::from_str(&args.payload)
        .with_context(|| format!("parsing --payload as JSON: {}", args.payload))?;

    let reply = client::send_signed(&kp, &args.endpoint, verb, payload).await?;
    println!("{}", serde_json::to_string_pretty(&reply.envelope.payload)?);
    Ok(())
}

fn parse_verb(s: &str) -> Result<ProtocolVerb> {
    match s {
        "ping" => Ok(ProtocolVerb::Ping),
        "describe_self" => Ok(ProtocolVerb::DescribeSelf),
        "get_known_peers" => Ok(ProtocolVerb::GetKnownPeers),
        "invoke" => Ok(ProtocolVerb::Invoke),
        other => anyhow::bail!("unknown verb: {other}"),
    }
}
