//! Discovery: bootstrap initial peers + transitive cascade + on-demand
//! capability cascade.
//!
//! - **Bootstrap (transitive)**: at startup, contact each configured initial
//!   peer, pull its `describe_self`, then walk that peer's
//!   `get_known_peers` up to `max_depth`. Each new peer found is itself
//!   refreshed. Bounded by `max_depth` and a hard cap on total
//!   discoveries to keep startup latency predictable.
//! - **Cascade**: on-demand, when looking for a specific capability, ask
//!   up to N random known peers for `get_known_peers(filter=capability)`.

use std::collections::HashSet;
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

/// Maximum number of random peers contacted per on-demand cascade.
const CASCADE_FAN_OUT: usize = 5;

/// Default transitive bootstrap depth. 0 = same behaviour as v0.2 (just
/// the configured seeds). 1 = seeds + their immediate known peers. 2 =
/// up to grand-peers. Higher = exponential fan-out — be careful.
pub const DEFAULT_BOOTSTRAP_DEPTH: u32 = 2;

/// Hard cap on the number of peers learned in a single bootstrap walk
/// (across all depths). Prevents pathological topologies from blowing up
/// startup time.
const BOOTSTRAP_MAX_PEERS: usize = 100;

/// Max peers requested from each `get_known_peers` call during bootstrap.
const BOOTSTRAP_PAGE_SIZE: u32 = 50;

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
///
/// This is the depth-0 entry point — same shape as v0.2. For the
/// transitive walk that also pulls each seed's known peers (recursively
/// up to `max_depth`), call [`bootstrap_transitive`].
pub async fn bootstrap_initial_peers(node: &Node, endpoints: &[String]) -> Vec<BootstrapOutcome> {
    if endpoints.is_empty() {
        return Vec::new();
    }
    info!(
        count = endpoints.len(),
        "bootstrapping initial peers (depth=0)"
    );
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

/// Transitive bootstrap: pull `describe_self` from each seed, then walk
/// `get_known_peers` up to `max_depth` levels deep, pulling
/// `describe_self` for every new peer learned. Bounded by
/// [`BOOTSTRAP_MAX_PEERS`] to keep startup latency predictable.
///
/// The first level (the seeds themselves) is reported verbatim in the
/// returned `Vec<BootstrapOutcome>` so callers can log seed-level
/// success/failure as before. Transitive discoveries are written to the
/// peer directory but not enumerated in the return value.
///
/// `max_depth = 0` → exactly equivalent to [`bootstrap_initial_peers`].
pub async fn bootstrap_transitive(
    node: &Node,
    seeds: &[String],
    max_depth: u32,
) -> Vec<BootstrapOutcome> {
    let seeds_outcomes = bootstrap_initial_peers(node, seeds).await;
    if max_depth == 0 {
        return seeds_outcomes;
    }

    info!(
        max_depth,
        cap = BOOTSTRAP_MAX_PEERS,
        "bootstrap: starting transitive walk"
    );

    let client = client::http_client();
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(node.instance_id().to_string());
    // Seed visited with whatever we just learned (seeds may not have
    // resolved cleanly; track endpoints to avoid re-trying).
    for o in &seeds_outcomes {
        if let Some(id) = &o.instance_id {
            visited.insert(id.clone());
        }
    }

    // Frontier holds endpoints to expand at the current depth.
    let mut frontier: Vec<String> = seeds_outcomes
        .iter()
        .filter(|o| o.instance_id.is_some())
        .map(|o| o.endpoint.clone())
        .collect();

    let mut total_discovered = seeds_outcomes.iter().filter(|o| o.error.is_none()).count();

    for depth in 1..=max_depth {
        if frontier.is_empty() || total_discovered >= BOOTSTRAP_MAX_PEERS {
            break;
        }
        debug!(
            depth,
            frontier_size = frontier.len(),
            "bootstrap walk: next level"
        );
        let mut next_frontier: Vec<String> = Vec::new();
        let req = GetKnownPeersRequest {
            limit: BOOTSTRAP_PAGE_SIZE,
            capability: None,
        };
        for endpoint in &frontier {
            let resp = match tokio::time::timeout(
                Duration::from_secs(10),
                client::get_known_peers(
                    &client,
                    node.keypair(),
                    endpoint,
                    req.clone(),
                    node.config().endpoint.as_deref(),
                ),
            )
            .await
            {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => {
                    warn!(endpoint = %endpoint, error = %e, "transitive bootstrap: get_known_peers failed");
                    continue;
                }
                Err(_) => {
                    warn!(endpoint = %endpoint, "transitive bootstrap: get_known_peers timed out");
                    continue;
                }
            };
            for summary in resp.peers {
                if total_discovered >= BOOTSTRAP_MAX_PEERS {
                    break;
                }
                let id_str = summary.instance_id.to_string();
                if !visited.insert(id_str.clone()) {
                    continue;
                }
                let Some(peer_endpoint) = summary.endpoint else {
                    continue;
                };
                match tokio::time::timeout(
                    Duration::from_secs(10),
                    refresh_peer(node, &client, &peer_endpoint),
                )
                .await
                {
                    Ok(Ok(_)) => {
                        total_discovered += 1;
                        debug!(peer = %id_str, endpoint = %peer_endpoint, "transitive bootstrap: cached peer");
                        next_frontier.push(peer_endpoint);
                    }
                    Ok(Err(e)) => {
                        warn!(peer = %id_str, error = %e, "transitive bootstrap: refresh_peer failed");
                    }
                    Err(_) => {
                        warn!(peer = %id_str, "transitive bootstrap: refresh_peer timed out");
                    }
                }
            }
        }
        frontier = next_frontier;
    }

    info!(
        learned = total_discovered,
        max_depth, "bootstrap: transitive walk complete"
    );
    seeds_outcomes
}

/// Fetch a peer's `describe_self`, upsert it into the local directory,
/// return the parsed descriptor.
pub async fn refresh_peer(
    node: &Node,
    client: &Client,
    endpoint: &str,
) -> NodeResult<DescribeSelfResponse> {
    let descriptor = client::describe_self(
        client,
        node.keypair(),
        endpoint,
        node.config().endpoint.as_deref(),
    )
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
        let mut rng = rand::rng();
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
            client::get_known_peers(
                &client,
                node.keypair(),
                &candidate.endpoint,
                req.clone(),
                node.config().endpoint.as_deref(),
            ),
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
