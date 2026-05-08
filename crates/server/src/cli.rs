use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;

use crate::http;

const DEFAULT_PORT: u16 = 4242;

fn default_config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(xdg).join("n3ur0n")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config/n3ur0n")
    } else {
        PathBuf::from(".n3ur0n")
    }
}

#[derive(Debug, Args)]
pub struct InitArgs {
    #[arg(long)]
    pub config_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ServeArgs {
    #[arg(long)]
    pub config_dir: Option<PathBuf>,

    #[arg(long, default_value_t = DEFAULT_PORT)]
    pub port: u16,
}

#[derive(Debug, Args)]
pub struct KeysArgs {
    #[arg(long)]
    pub config_dir: Option<PathBuf>,
}

pub async fn init(args: InitArgs) -> Result<()> {
    let dir = args.config_dir.unwrap_or_else(default_config_dir);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating config dir {}", dir.display()))?;

    let key_path = dir.join("keys.json");
    if key_path.exists() {
        anyhow::bail!("keys already present at {}", key_path.display());
    }

    let keypair = n3ur0n_core::Keypair::generate();
    let secret = data_encoding::HEXLOWER.encode(&keypair.secret_bytes());
    let public = data_encoding::HEXLOWER.encode(keypair.public_key().0.as_bytes());
    let json = serde_json::json!({
        "instance_id": keypair.instance_id().as_str(),
        "secret_hex": secret,
        "public_hex": public,
    });
    std::fs::write(&key_path, serde_json::to_string_pretty(&json)?)
        .with_context(|| format!("writing {}", key_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(&key_path)?.permissions();
        perm.set_mode(0o600);
        std::fs::set_permissions(&key_path, perm)?;
    }

    let db_path = dir.join("n3ur0n.sqlite");
    let _ = n3ur0n_storage::open(&db_path)?;

    println!("instance id: {}", keypair.instance_id());
    println!("config dir : {}", dir.display());
    println!("keys       : {}", key_path.display());
    println!("database   : {}", db_path.display());
    Ok(())
}

pub async fn serve(args: ServeArgs) -> Result<()> {
    let dir = args.config_dir.unwrap_or_else(default_config_dir);
    let db_path = dir.join("n3ur0n.sqlite");
    let db = n3ur0n_storage::open(&db_path)
        .with_context(|| format!("opening db at {}", db_path.display()))?;

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], args.port));
    tracing::info!(port = args.port, "starting n3ur0n server");
    http::serve(addr, db).await
}

pub async fn keys(args: KeysArgs) -> Result<()> {
    let dir = args.config_dir.unwrap_or_else(default_config_dir);
    let key_path = dir.join("keys.json");
    let raw = std::fs::read_to_string(&key_path)
        .with_context(|| format!("reading {}", key_path.display()))?;
    let v: serde_json::Value = serde_json::from_str(&raw)?;
    println!("{}", v["instance_id"].as_str().unwrap_or(""));
    Ok(())
}
