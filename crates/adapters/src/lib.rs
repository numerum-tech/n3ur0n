//! n3ur0n-adapters
//!
//! Backend trait + concrete implementations selected by config. Spec §7.

use async_trait::async_trait;
use n3ur0n_core::capability::CapabilityDecl;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("capability not found: {0}")]
    UnknownCapability(String),

    #[error("backend transport: {0}")]
    Transport(String),

    #[error("backend returned error: {0}")]
    Backend(String),

    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
}

pub type AdapterResult<T> = Result<T, AdapterError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

#[async_trait]
pub trait Backend: Send + Sync {
    async fn invoke(&self, capability: &str, args: Value) -> AdapterResult<Value>;
    async fn describe(&self) -> AdapterResult<Vec<CapabilityDecl>>;
    async fn health(&self) -> AdapterResult<HealthStatus>;
}

pub mod echo;
pub mod openai;
