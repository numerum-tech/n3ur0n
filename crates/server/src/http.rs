use std::net::SocketAddr;

use anyhow::Result;
use axum::{Json, Router, routing::get};
use n3ur0n_storage::Db;
use serde_json::json;

#[derive(Clone)]
struct AppState {
    #[allow(dead_code)]
    db: Db,
}

pub async fn serve(addr: SocketAddr, db: Db) -> Result<()> {
    let state = AppState { db };

    let api_v0 = Router::new().route("/health", get(health));

    // Stub: /n3ur0n/v0 will host signature-verifying middleware + protocol verbs.
    let proto_v0 = Router::new().route("/health", get(health));

    let app = Router::new()
        .nest("/api/v0", api_v0)
        .nest("/n3ur0n/v0", proto_v0)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}
