//! Discovery: bootstrap initial peers + on-demand cascade depth-1.
//!
//! v0.1 strategy (architecture spec §8.5):
//! - **Bootstrap**: at startup, contact each configured initial peer, fetch
//!   its signed `describe_self`, store the result in the local directory.
//! - **Cascade**: when looking for a capability, ask up to N random known
//!   peers for `get_known_peers(filter=capability)`, then for each new id
//!   pull `describe_self` and add it to the directory. No multi-hop.

use std::time::Duration;

use n3ur0n_core::protocol::{DescribeSelfResponse, GetKnownPeersRequest};
use n3ur0n_storage::peers::{self, PeerRecord};
use rand::seq::SliceRandom;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::client;
use crate::error::{NodeError, NodeResult};
use crate::node::Node;

/// Maximum number of random peers contacted per cascade.
const CASCADE_FAN_OUT: usize = 5;

/// Result of a single bootstrap attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapOutcome {
    pub endpoint: String,
    pub instance_id: Option<String>,
    pub error: Option<String>,
}

/// Contact each `endpoint`, fetch its signed `describe_self`, persist it.
/// Failures are non-fatal: a bad endpoint must not prevent the others from
/// being tried.
pub async fn bootstrap_initial_peers(
    node: &Node,
    endpoints: &[String],
) -> Vec<BootstrapOutcome> {
    if endpoints.is_empty() {
        return Vec::new();
    }
    info!(count = endpoints.len(), "bootstrapping initial peers");
    let client = client::http_client();
    let mut out = Vec::with_capacity(endpoints.len());
    for ep in endpoints {
        let outcome = match refresh_peer(node, &client, ep).await {
            Ok(desc) => BootstrapOutcome {
                endpoint: ep.clone(),
                instance_id: Some(desc.instance_id.to_string()),
                error: None,
            },
            Err(e) => {
                warn!(endpoint = %ep, error = %e, "bootstrap failed for endpoint");
                BootstrapOutcome {
                    endpoint: ep.clone(),
                    instance_id: None,
                    error: Some(e.to_string()),
                }
            }
        };
        out.push(outcome);
    }
    out
}

/// Fetch a peer's `describe_self`, upsert it into the local directory,
/// return the parsed descriptor.
pub async fn refresh_peer(
    node: &Node,
    client: &Client,
    endpoint: &str,
) -> NodeResult<DescribeSelfResponse> {
    let descriptor = client::describe_self(client, node.keypair(), endpoint)
        .await
        .map_err(|e| NodeError::InvalidPayload(format!("describe_self {endpoint}: {e}")))?;
    let now = node.clock().now().unix_timestamp();
    let cached = serde_json::to_string(&descriptor)?;
    let record = PeerRecord {
        id: descriptor.instance_id.to_string(),
        endpoint: endpoint.to_string(),
        alias: descriptor.alias.clone(),
        last_seen: Some(now),
        tls_fingerprint: None,
        describe_self_cached: Some(cached),
        describe_self_fetched_at: Some(now),
        source: Some("bootstrap".into()),
    };
    peers::upsert(node.db(), &record)?;
    Ok(descriptor)
}

/// Cascade discovery (depth 1) for a capability name.
///
/// Picks up to [`CASCADE_FAN_OUT`] random known peers, asks each for
/// `get_known_peers(filter=capability)`, then pulls `describe_self` for any
/// returned id we don't already have locally. Records discovered peers in
/// the directory and returns the count of new entries.
pub async fn discover_capability(node: &Node, capability: &str) -> NodeResult<usize> {
    let local_peers = peers::list(node.db(), 1000)?;
    if local_peers.is_empty() {
        debug!(capability, "no known peers; cascade is a no-op");
        return Ok(0);
    }

    // ThreadRng is !Send; scope it so the future stays Send across awaits.
    let sample: Vec<&PeerRecord> = {
        let mut rng = rand::thread_rng();
        let mut s: Vec<&PeerRecord> = local_peers.iter().collect();
        s.shuffle(&mut rng);
        s.truncate(CASCADE_FAN_OUT);
        s
    };

    let client = client::http_client();
    let req = GetKnownPeersRequest {
        limit: 100,
        capability: Some(capability.to_string()),
    };

    let mut new_count = 0usize;
    let timeout = Duration::from_secs(10);

    for candidate in sample {
        let resp = match tokio::time::timeout(
            timeout,
            client::get_known_peers(&client, node.keypair(), &candidate.endpoint, req.clone()),
        )
        .await
        {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                warn!(peer = %candidate.id, error = %e, "get_known_peers failed");
                continue;
            }
            Err(_) => {
                warn!(peer = %candidate.id, "get_known_peers timed out");
                continue;
            }
        };

        for summary in resp.peers {
            let id_str = summary.instance_id.to_string();
            if id_str == node.instance_id().as_str() {
                continue;
            }
            if peers::get(node.db(), &id_str)?.is_some() {
                continue;
            }
            let Some(endpoint) = summary.endpoint else {
                debug!(peer = %id_str, "skipping discovered peer without endpoint");
                continue;
            };
            match tokio::time::timeout(timeout, refresh_peer(node, &client, &endpoint)).await {
                Ok(Ok(_)) => new_count += 1,
                Ok(Err(e)) => warn!(peer = %id_str, error = %e, "refresh_peer failed"),
                Err(_) => warn!(peer = %id_str, "refresh_peer timed out"),
            }
        }
    }

    Ok(new_count)
}
