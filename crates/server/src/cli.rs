//! CLI subcommands.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use std::sync::Arc;

use n3ur0n_adapters::openai::OpenAIConfig;
use n3ur0n_core::message::ProtocolVerb;
use n3ur0n_node::IdentityFile;
use n3ur0n_node::client as peer_client;
use n3ur0n_node::discovery;
use n3ur0n_node::runtime::RuntimeConfig;
use arc_swap::ArcSwap;
use n3ur0n_server::bootstrap::{self, BackendKind, PlannerKind};
use n3ur0n_server::http;
use n3ur0n_server::planner_config::{PlannerEnvFallback, load_planner_user_config};
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

    /// Transitive bootstrap depth. 0 = pull only the seeds themselves
    /// (legacy v0.2 behaviour). 1 = seeds + their immediate known peers.
    /// 2 = up to grand-peers (default). Higher values walk further but
    /// fan-out grows; capped at 100 peers total.
    #[arg(long = "bootstrap-depth", env = "N3UR0N_BOOTSTRAP_DEPTH", default_value_t = 2)]
    pub(crate) bootstrap_depth: u32,

    /// Backend adapter: `echo` (default) or `openai` (also covers Ollama,
    /// llama.cpp, vLLM, etc.).
    #[arg(long, env = "N3UR0N_BACKEND", default_value = "echo")]
    pub(crate) backend: String,

    /// Base URL of the OpenAI-compatible endpoint when `--backend openai`.
    /// e.g. http://host.docker.internal:11434 (Ollama from a container).
    #[arg(long, env = "N3UR0N_OPENAI_BASE_URL")]
    pub(crate) openai_base_url: Option<String>,

    /// Model name to send by default when `--backend openai`.
    #[arg(long, env = "N3UR0N_OPENAI_MODEL")]
    pub(crate) openai_model: Option<String>,

    /// Bearer token for the OpenAI-compatible endpoint (optional).
    #[arg(long, env = "N3UR0N_OPENAI_API_KEY", hide_env_values = true)]
    pub(crate) openai_api_key: Option<String>,

    /// v0.3 manifest mode: scan `<dir>/backends/*.toml` and
    /// `<dir>/caps/*.toml` at startup; each capability carries its own
    /// binding (prompt / http / mcp). When set, the `--backend` flag is
    /// ignored.
    #[arg(long = "manifest-dir", env = "N3UR0N_MANIFEST_DIR")]
    pub(crate) manifest_dir: Option<PathBuf>,

    /// Planner selector. `none` = no planner (manual mode only); `llm` =
    /// LLM-driven planner using the `--planner-llm-*` flags below.
    #[arg(long, env = "N3UR0N_PLANNER_MODE", default_value = "none")]
    pub(crate) planner_mode: String,

    /// Base URL of the LLM endpoint used by the planner (required when
    /// `N3UR0N_PLANNER_MODE` is not `none`).
    #[arg(long, env = "N3UR0N_PLANNER_LLM_BASE_URL")]
    pub(crate) planner_llm_base_url: Option<String>,

    /// Model identifier for the planner LLM (e.g. `llama3.1:8b`).
    #[arg(long, env = "N3UR0N_PLANNER_LLM_MODEL")]
    pub(crate) planner_llm_model: Option<String>,

    /// Bearer token for the planner LLM endpoint (optional).
    #[arg(long, env = "N3UR0N_PLANNER_LLM_API_KEY", hide_env_values = true)]
    pub(crate) planner_llm_api_key: Option<String>,

    /// Max concurrent planner dispatches (semaphore).
    #[arg(long, env = "N3UR0N_MAX_CONCURRENT_PLANNERS", default_value_t = 4)]
    pub(crate) max_concurrent_planners: usize,

    /// Max active conversations in LRU cache.
    #[arg(long, env = "N3UR0N_MAX_ACTIVE_CONVERSATIONS", default_value_t = 50)]
    pub(crate) max_active_conversations: usize,
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
    let cli_peers: Vec<String> = args
        .bootstrap
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let bootstrap_peers =
        n3ur0n_server::bootstrap_config::resolve_startup_peers(&cli_peers, &dir);

    // v0.3 manifest mode trumps the compile-time --backend selector.
    let backend_kind = if let Some(manifest_dir) = args.manifest_dir.clone() {
        tracing::info!(
            manifest_dir = %manifest_dir.display(),
            "manifest mode active; --backend flag ignored"
        );
        bootstrap::BackendKind::Manifest { dir: manifest_dir }
    } else {
        parse_backend_kind(
            &args.backend,
            args.openai_base_url.clone(),
            args.openai_model.clone(),
            args.openai_api_key.clone(),
        )?
    };
    let node = bootstrap::load_node(&dir, args.endpoint, bootstrap_peers.clone(), backend_kind).await?;
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], args.port));
    tracing::info!(instance_id = %node.instance_id(), port = args.port, "starting n3ur0n server");

    if !bootstrap_peers.is_empty() {
        let bg = node.clone();
        let depth = args.bootstrap_depth;
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let outcomes =
                n3ur0n_node::discovery::bootstrap_transitive(&bg, &bootstrap_peers, depth).await;
            for o in &outcomes {
                match (&o.instance_id, &o.error) {
                    (Some(id), _) => tracing::info!(endpoint = %o.endpoint, peer = %id, "bootstrap ok"),
                    (None, Some(err)) => tracing::warn!(endpoint = %o.endpoint, error = %err, "bootstrap failed"),
                    _ => {}
                }
            }
        });
    }

    // Optional planner runtime.
    let planner_kind = parse_planner_kind(
        &args.planner_mode,
        args.planner_llm_base_url.clone(),
        args.planner_llm_model.clone(),
        args.planner_llm_api_key.clone(),
    )?;
    let runtime_config = RuntimeConfig {
        max_concurrent_planners: args.max_concurrent_planners,
        max_active_conversations: args.max_active_conversations,
    };
    let planner_env = planner_kind.as_ref().map(|kind| {
        let PlannerKind::PlanExec { backend, model_hint } = kind;
        PlannerEnvFallback {
            base_url: backend.base_url.clone(),
            default_model: model_hint
                .clone()
                .unwrap_or_else(|| backend.default_model.clone()),
            api_key: backend.api_key.clone(),
        }
    });
    let runtime_cell = Arc::new(ArcSwap::from_pointee(None));
    if let Some(env) = planner_env.as_ref() {
        let user = load_planner_user_config(&dir);
        let rt = bootstrap::build_runtime_with_user_config(
            node.clone(),
            &dir,
            env,
            &user,
            runtime_config.clone(),
        )?;
        runtime_cell.store(Arc::new(Some(Arc::new(rt))));
        tracing::info!("planner runtime configured");
    } else {
        tracing::info!("no planner configured (manual mode only)");
    }

    let blobs_root = dir.join("blobs").join("sha256");
    std::fs::create_dir_all(&blobs_root)
        .with_context(|| format!("creating blob dir {}", blobs_root.display()))?;
    n3ur0n_server::blob_gc::spawn(node.clone(), blobs_root);

    let app = http::app_with_settings(
        node,
        runtime_cell,
        Some(dir.clone()),
        planner_env.clone(),
        planner_env.map(|_| runtime_config),
    );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");
    axum::serve(listener, app).await?;
    Ok(())
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
    // CLI `send` does not run a server, so no sender_endpoint to advertise.
    let reply = peer_client::send_signed(&client, &kp, &args.endpoint, verb, payload, None).await?;
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
    let node = bootstrap::load_node(&dir, None, vec![], BackendKind::Echo).await?;
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
    let node = bootstrap::load_node(&dir, None, vec![], BackendKind::Echo).await?;
    let added = discovery::discover_capability(&node, &capability).await?;
    println!("discovered {added} new peer(s) for capability \"{capability}\"");
    Ok(())
}

fn parse_backend_kind(
    name: &str,
    openai_base_url: Option<String>,
    openai_model: Option<String>,
    openai_api_key: Option<String>,
) -> Result<BackendKind> {
    match name.to_ascii_lowercase().as_str() {
        "echo" => Ok(BackendKind::Echo),
        "utility" | "util" | "tools" => Ok(BackendKind::Utility),
        "openai" | "ollama" => {
            let base_url = openai_base_url
                .or_else(|| {
                    if name.eq_ignore_ascii_case("ollama") {
                        Some("http://localhost:11434".into())
                    } else {
                        None
                    }
                })
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "--openai-base-url (or env N3UR0N_OPENAI_BASE_URL) required when --backend openai"
                    )
                })?;
            let default_model = openai_model.ok_or_else(|| {
                anyhow::anyhow!(
                    "--openai-model (or env N3UR0N_OPENAI_MODEL) required when --backend openai"
                )
            })?;
            Ok(BackendKind::OpenAI(OpenAIConfig {
                base_url,
                default_model,
                api_key: openai_api_key,
                description: None,
                allow_model_override: false,
            }))
        }
        other => anyhow::bail!("unknown backend: {other}"),
    }
}

fn parse_planner_kind(
    name: &str,
    base_url: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
) -> Result<Option<PlannerKind>> {
    match name.to_ascii_lowercase().as_str() {
        "none" | "" | "off" => Ok(None),
        "plan_exec" | "plan-exec" | "plan" | "ptx" | "llm" | "ollama" => {
            // v0.2 deprecates the standalone ReAct planner. Legacy mode
            // names ("llm", "ollama") resolve to PlanExec for
            // backwards compatibility with existing configs.
            let backend = build_openai_config(base_url, model.clone(), api_key)?;
            Ok(Some(PlannerKind::PlanExec {
                backend,
                model_hint: model,
            }))
        }
        other => anyhow::bail!("unknown planner mode: {other}"),
    }
}

fn build_openai_config(
    base_url: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
) -> Result<OpenAIConfig> {
    let base_url = base_url.ok_or_else(|| {
        anyhow::anyhow!(
            "--planner-llm-base-url (or env N3UR0N_PLANNER_LLM_BASE_URL) required for LLM-backed planner"
        )
    })?;
    let model = model.ok_or_else(|| {
        anyhow::anyhow!(
            "--planner-llm-model (or env N3UR0N_PLANNER_LLM_MODEL) required for LLM-backed planner"
        )
    })?;
    Ok(OpenAIConfig {
        base_url,
        default_model: model,
        api_key,
        description: None,
        allow_model_override: false,
    })
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
