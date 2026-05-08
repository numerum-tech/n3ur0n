//! CLI subcommands.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use n3ur0n_core::message::ProtocolVerb;
use n3ur0n_node::IdentityFile;
use n3ur0n_node::client as peer_client;
use n3ur0n_node::discovery;
use n3ur0n_server::{bootstrap, http};
use n3ur0n_storage::peers as peers_repo;
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

    /// Initial peers to bootstrap from. Repeatable; comma-separated values
    /// also accepted via the `N3UR0N_BOOTSTRAP_PEERS` env variable.
    #[arg(long = "bootstrap", env = "N3UR0N_BOOTSTRAP_PEERS", value_delimiter = ',', num_args = 0..)]
    pub(crate) bootstrap: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct KeysArgs {
    #[arg(long, env = "N3UR0N_CONFIG_DIR")]
    pub(crate) config_dir: Option<PathBuf>,
}

#[derive(Debug, clap::Subcommand)]
pub(crate) enum PeersAction {
    /// List peers in the local directory.
    List {
        #[arg(long, env = "N3UR0N_CONFIG_DIR")]
        config_dir: Option<PathBuf>,

        #[arg(long, default_value_t = 50)]
        limit: i64,
    },

    /// Pull `describe_self` from a remote endpoint and upsert into directory.
    Refresh {
        #[arg(long, env = "N3UR0N_CONFIG_DIR")]
        config_dir: Option<PathBuf>,

        #[arg(long)]
        endpoint: String,
    },

    /// Cascade-discover peers offering a capability.
    Discover {
        #[arg(long, env = "N3UR0N_CONFIG_DIR")]
        config_dir: Option<PathBuf>,

        #[arg(long)]
        capability: String,
    },
}

#[derive(Debug, Args)]
pub(crate) struct PeersArgs {
    #[command(subcommand)]
    pub(crate) action: PeersAction,
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
    let bootstrap_peers: Vec<String> = args
        .bootstrap
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let node = bootstrap::load_node(&dir, args.endpoint, bootstrap_peers.clone()).await?;
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], args.port));
    tracing::info!(instance_id = %node.instance_id(), port = args.port, "starting n3ur0n server");

    if !bootstrap_peers.is_empty() {
        let bg = node.clone();
        tokio::spawn(async move {
            // Tiny delay so our own listener is up before we attempt the first
            // outbound (bootstrap peers may be the same compose stack restarting).
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let outcomes = n3ur0n_node::discovery::bootstrap_initial_peers(&bg, &bootstrap_peers).await;
            for o in &outcomes {
                match (&o.instance_id, &o.error) {
                    (Some(id), _) => tracing::info!(endpoint = %o.endpoint, peer = %id, "bootstrap ok"),
                    (None, Some(err)) => tracing::warn!(endpoint = %o.endpoint, error = %err, "bootstrap failed"),
                    _ => {}
                }
            }
        });
    }

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

    let client = peer_client::http_client();
    let reply = peer_client::send_signed(&client, &kp, &args.endpoint, verb, payload).await?;
    println!("{}", serde_json::to_string_pretty(&reply.envelope.payload)?);
    Ok(())
}

pub(crate) async fn peers(args: PeersArgs) -> Result<()> {
    match args.action {
        PeersAction::List { config_dir, limit } => peers_list(config_dir, limit).await,
        PeersAction::Refresh { config_dir, endpoint } => peers_refresh(config_dir, endpoint).await,
        PeersAction::Discover {
            config_dir,
            capability,
        } => peers_discover(config_dir, capability).await,
    }
}

async fn peers_list(config_dir: Option<PathBuf>, limit: i64) -> Result<()> {
    let dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let db = n3ur0n_storage::open(bootstrap::db_path(&dir))?;
    let rows = peers_repo::list(&db, limit)?;
    if rows.is_empty() {
        println!("(no peers in directory)");
        return Ok(());
    }
    for p in rows {
        println!(
            "{}\t{}\t{}",
            p.id,
            p.endpoint,
            p.alias.unwrap_or_else(|| "-".into())
        );
    }
    Ok(())
}

async fn peers_refresh(config_dir: Option<PathBuf>, endpoint: String) -> Result<()> {
    let dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let node = bootstrap::load_node(&dir, None, vec![]).await?;
    let client = peer_client::http_client();
    let desc = discovery::refresh_peer(&node, &client, &endpoint).await?;
    println!(
        "{}\t{}\tcaps: {:?}",
        desc.instance_id,
        desc.endpoint.unwrap_or_else(|| "-".into()),
        desc.capabilities.iter().map(|c| &c.name).collect::<Vec<_>>()
    );
    Ok(())
}

async fn peers_discover(config_dir: Option<PathBuf>, capability: String) -> Result<()> {
    let dir = config_dir.unwrap_or_else(bootstrap::default_config_dir);
    let node = bootstrap::load_node(&dir, None, vec![]).await?;
    let added = discovery::discover_capability(&node, &capability).await?;
    println!("discovered {added} new peer(s) for capability \"{capability}\"");
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
