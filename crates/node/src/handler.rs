//! Verb dispatcher.
//!
//! Single entry point: [`handle_request`] takes an inbound [`SignedMessage`],
//! verifies it, dispatches to the verb-specific handler, and produces a signed
//! response addressed to the original sender.

use n3ur0n_core::capability::AccessMode;
use n3ur0n_core::message::{Envelope, ProtocolVerb, SignedMessage};
use n3ur0n_core::protocol::{
    DescribeSelfResponse, GetKnownPeersRequest, GetKnownPeersResponse, InvokeRequest,
    InvokeResponse, PROTOCOL_VERSION, PeerSummary, PingResponse,
};
use n3ur0n_core::verify::verify_envelope;
use n3ur0n_storage::{nonces, peers};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::{NodeError, NodeResult};
use crate::node::Node;

/// Anti-replay window in seconds. Architecture spec recommends 1 hour.
const NONCE_TTL_SECONDS: i64 = 60 * 60;

/// Process an inbound `SignedMessage` end-to-end.
///
/// Steps: verify (signature/binding/recipient/clock) → anti-replay → dispatch
/// to verb handler → wrap response in a signed envelope.
pub async fn handle_request(node: &Node, request: SignedMessage) -> NodeResult<SignedMessage> {
    let verified = verify_envelope(
        request,
        &node.instance_id(),
        node.clock().as_ref(),
        &node.config().verify,
    )?;
    let inbound = verified.message;

    let now = node.clock().now();
    let now_secs = now.unix_timestamp();
    let inserted = nonces::insert_if_absent(
        node.db(),
        inbound.envelope.sender_id.as_str(),
        &inbound.envelope.nonce,
        now_secs,
    )?;
    if !inserted {
        return Err(NodeError::Replay);
    }
    // Best-effort prune of expired nonces. Failures here are not fatal.
    if let Err(e) = nonces::prune_older_than(node.db(), now_secs - NONCE_TTL_SECONDS) {
        tracing::warn!(error = %e, "nonce prune failed");
    }

    let response_payload = match inbound.envelope.verb {
        ProtocolVerb::DescribeSelf => describe_self(node, now)?,
        ProtocolVerb::Ping => ping(now)?,
        ProtocolVerb::GetKnownPeers => get_known_peers(node, &inbound.envelope.payload)?,
        ProtocolVerb::Invoke => invoke(node, &inbound.envelope).await?,
        ProtocolVerb::BlobTicket => {
            return Err(NodeError::InvalidPayload(
                "blob_ticket is not accepted on /messages; use /n3ur0n/v0/blobs".into(),
            ));
        }
    };

    // Reverse-announce: if the inbound envelope carried a `sender_endpoint`
    // we trust the signed claim (TOFU) and upsert the caller's contact
    // info into our peer directory. From now on we can call them back.
    // Self-calls are silently ignored. Errors are logged but never block
    // the reply.
    if let Some(claimed_endpoint) = &inbound.envelope.sender_endpoint
        && inbound.envelope.sender_id != node.instance_id()
        && !claimed_endpoint.is_empty()
    {
        // Fast path: upsert (id, endpoint) so we know where the caller
        // lives. We do NOT block the reply on a describe_self
        // round-trip — that runs asynchronously in the background and
        // fills the caps cache on next read.
        let already_known =
            n3ur0n_storage::peers::get(node.db(), &inbound.envelope.sender_id.to_string())
                .ok()
                .flatten()
                .is_some();
        let now_unix = now.unix_timestamp();
        let record = n3ur0n_storage::peers::PeerRecord {
            id: inbound.envelope.sender_id.to_string(),
            endpoint: claimed_endpoint.clone(),
            alias: None,
            last_seen: Some(now_unix),
            tls_fingerprint: None,
            describe_self_cached: None,
            describe_self_fetched_at: None,
            source: Some("reverse_announce".into()),
        };
        if let Err(e) = n3ur0n_storage::peers::upsert(node.db(), &record) {
            tracing::warn!(
                error = %e,
                peer = %inbound.envelope.sender_id,
                "reverse-announce upsert failed"
            );
        } else {
            tracing::debug!(
                peer = %inbound.envelope.sender_id,
                endpoint = %claimed_endpoint,
                "reverse-announce: cached caller"
            );
            // Async describe_self pull on first sighting so caps
            // surface in the UI without a manual refresh. Skipped
            // when the peer was already known to avoid hammering
            // upstreams on every signed call.
            if !already_known {
                let node_for_pull = node.clone();
                let ep = claimed_endpoint.clone();
                tokio::spawn(async move {
                    let client = crate::client::http_client();
                    if let Err(e) =
                        crate::discovery::refresh_peer(&node_for_pull, &client, &ep).await
                    {
                        tracing::debug!(
                            endpoint = %ep,
                            error = %e,
                            "reverse-announce: background describe_self failed (will retry on next call)"
                        );
                    }
                });
            }
        }
    }

    let reply = Envelope {
        sender_id: node.instance_id(),
        recipient_id: inbound.envelope.sender_id.clone(),
        timestamp: now,
        nonce: Uuid::new_v4().to_string(),
        verb: inbound.envelope.verb,
        payload: response_payload,
        // Reply also advertises *our* endpoint so the caller (initiator)
        // can refresh their cache transparently.
        sender_endpoint: node.config().endpoint.clone(),
    };
    Ok(reply.sign(node.keypair())?)
}

fn describe_self(node: &Node, now: OffsetDateTime) -> NodeResult<Value> {
    let body = DescribeSelfResponse {
        instance_id: node.instance_id(),
        endpoint: node.config().endpoint.clone(),
        alias: node.config().alias.clone(),
        protocol_version: PROTOCOL_VERSION.into(),
        updated_at: now
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| NodeError::InvalidPayload(e.to_string()))?,
        capabilities: node.registry().public_decls(),
    };
    Ok(serde_json::to_value(body)?)
}

fn ping(now: OffsetDateTime) -> NodeResult<Value> {
    let body = PingResponse {
        server_time: now
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|e| NodeError::InvalidPayload(e.to_string()))?,
    };
    Ok(serde_json::to_value(body)?)
}

fn get_known_peers(node: &Node, payload: &Value) -> NodeResult<Value> {
    let req: GetKnownPeersRequest = serde_json::from_value(payload.clone())
        .map_err(|e| NodeError::InvalidPayload(format!("get_known_peers: {e}")))?;

    let limit = i64::from(req.limit.min(1000));
    let records = peers::list(node.db(), limit)?;

    let summaries = records
        .into_iter()
        .filter_map(|p| {
            let id = match n3ur0n_core::InstanceId::parse(&p.id) {
                Ok(v) => v,
                Err(_) => return None,
            };
            // Capability filter: best-effort against the cached describe_self
            // blob. Peers without a cached descriptor are *not* filtered out
            // when no filter is requested, but are excluded when a filter is.
            if let Some(want) = &req.capability {
                let cached: Option<DescribeSelfResponse> = p
                    .describe_self_cached
                    .as_deref()
                    .and_then(|raw| serde_json::from_str(raw).ok());
                let matches = cached
                    .map(|d| {
                        d.capabilities
                            .iter()
                            .any(|c| &c.name == want && c.mode.is_public())
                    })
                    .unwrap_or(false);
                if !matches {
                    return None;
                }
            }
            Some(PeerSummary {
                instance_id: id,
                endpoint: Some(p.endpoint),
                alias: p.alias,
            })
        })
        .collect();

    let body = GetKnownPeersResponse { peers: summaries };
    Ok(serde_json::to_value(body)?)
}

async fn invoke(node: &Node, envelope: &Envelope) -> NodeResult<Value> {
    let req: InvokeRequest = serde_json::from_value(envelope.payload.clone())
        .map_err(|e| NodeError::InvalidPayload(format!("invoke: {e}")))?;

    // Hold the registry snapshot for the whole call so the &CapabilityDecl
    // borrow stays valid even if a concurrent reload swaps the underlying
    // Arc.
    let registry = node.registry();
    let decl = registry
        .get(&req.capability)
        .ok_or_else(|| NodeError::UnknownCapability(req.capability.clone()))?;

    // Private caps are local-only: do not disclose existence to peers.
    if matches!(decl.mode, AccessMode::Private) {
        return Err(NodeError::UnknownCapability(req.capability.clone()));
    }

    if matches!(decl.mode, AccessMode::Restricted) && req.subscription_token.is_none() {
        return Err(NodeError::InvalidPayload(format!(
            "capability {} is restricted; subscription_token required",
            req.capability
        )));
    }
    // v0.1: subscription token validation is the operator's concern (out-of-band).

    // v0.3: bindings take precedence over the legacy single-backend path.
    // When the registry carries a binding for this cap (manifest mode),
    // dispatch through it; otherwise fall back to the compile-time
    // backend wired into the Node.
    let result = if let Some(binding) = registry.binding_for(&req.capability) {
        binding.invoke(req.args).await?
    } else {
        node.backend().invoke(&req.capability, req.args).await?
    };
    let body = InvokeResponse { result };
    Ok(serde_json::to_value(body)?)
}
