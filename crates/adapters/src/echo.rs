//! Echo backend: returns the args verbatim. For dev / smoke tests.

use async_trait::async_trait;
use n3ur0n_core::capability::{AccessMode, CapabilityDecl};
use serde_json::{Value, json};

use crate::{AdapterResult, Backend, HealthStatus};

pub struct EchoBackend;

#[async_trait]
impl Backend for EchoBackend {
    async fn invoke(&self, _capability: &str, args: Value) -> AdapterResult<Value> {
        Ok(args)
    }

    async fn describe(&self) -> AdapterResult<Vec<CapabilityDecl>> {
        Ok(vec![CapabilityDecl {
            name: "echo".into(),
            description: "Returns its input untouched.".into(),
            schema_in: json!({"type": "object"}),
            schema_out: json!({"type": "object"}),
            mode: AccessMode::Free,
            pricing: None,
            tags: vec!["debug".into()],
            lobe_ids: vec![],
        }])
    }

    async fn health(&self) -> AdapterResult<HealthStatus> {
        Ok(HealthStatus::Healthy)
    }
}
