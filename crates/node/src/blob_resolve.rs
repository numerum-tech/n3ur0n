//! Resolve BlobRefs before remote invokes and fetch output blobs after.

use std::path::PathBuf;

use n3ur0n_core::blob::{
    classify_inbound_output, classify_local_cache, classify_outbound_upload, hash_bytes,
    validate_hash, BlobRef,
};
use n3ur0n_storage::blobs::{self, BlobInsert};
use reqwest::Client;
use serde_json::Value;

use crate::blob_client::{download_blob, upload_blob};
use crate::client::discover_recipient;
use crate::node::Node;

/// Walk a JSON value and collect all embedded BlobRefs.
pub fn collect_blob_refs(value: &Value) -> Vec<BlobRef> {
    let mut out = Vec::new();
    walk_blob_refs(value, &mut out);
    out
}

fn walk_blob_refs(value: &Value, out: &mut Vec<BlobRef>) {
    if let Some(br) = parse_blob_ref(value) {
        out.push(br);
        return;
    }
    match value {
        Value::Object(map) => {
            for v in map.values() {
                walk_blob_refs(v, out);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                walk_blob_refs(v, out);
            }
        }
        _ => {}
    }
}

fn parse_blob_ref(value: &Value) -> Option<BlobRef> {
    let hash = value.get("hash")?.as_str()?;
    if validate_hash(hash).is_err() {
        return None;
    }
    let size = value.get("size")?.as_u64()?;
    let mime = value.get("mime")?.as_str()?;
    Some(BlobRef {
        hash: hash.to_string(),
        size,
        mime: mime.to_string(),
        fetch_url: value
            .get("fetch_url")
            .and_then(|u| u.as_str())
            .map(String::from),
    })
}

fn blobs_dir(node: &Node) -> Option<PathBuf> {
    node.config().blobs_dir.clone()
}

/// Read blob bytes from the local index path or `<blobs_dir>/<hash>`.
pub fn read_local_bytes(node: &Node, hash: &str) -> Option<Vec<u8>> {
    if let Ok(Some(rec)) = blobs::get(node.db(), hash) {
        if let Ok(b) = std::fs::read(&rec.storage_path) {
            if hash_bytes(&b) == hash {
                return Some(b);
            }
        }
    }
    let root = blobs_dir(node)?;
    let path = root.join(hash);
    std::fs::read(&path).ok().filter(|b| hash_bytes(b) == hash)
}

fn storage_path_for(node: &Node, hash: &str) -> Option<PathBuf> {
    blobs_dir(node).map(|d| d.join(hash))
}

fn record_outbound_upload(
    node: &Node,
    hash: &str,
    size: i64,
    mime: &str,
    path: &std::path::Path,
    local_user_id: Option<i64>,
    client_id: Option<&str>,
) -> Result<(), String> {
    let class = classify_outbound_upload();
    let now = node.clock().now().unix_timestamp();
    let expires = now + n3ur0n_core::default_ttl_secs(n3ur0n_core::BlobPurpose::Input) as i64;
    let row = BlobInsert {
        hash: hash.to_string(),
        size,
        mime: mime.to_string(),
        expires_at: expires,
        storage_path: path.display().to_string(),
        provenance: "outbound".into(),
        role: "input".into(),
        anchor_kind: "user_session".into(),
        processing_status: "referenced".into(),
        local_user_id,
        client_id: client_id.map(String::from),
        conversation_id: None,
        dispatch_id: None,
        capability: None,
        remote_sender_id: None,
        ticket_nonce: None,
        invoke_id: None,
        user_visible: class.user_visible,
        user_deletable: class.user_deletable,
        uploader_id: Some(node.instance_id().to_string()),
        recipients_whitelist: None,
    };
    blobs::upsert(node.db(), &row).map_err(|e| e.to_string())
}

fn record_inbound_output(
    node: &Node,
    blob: &BlobRef,
    path: &std::path::Path,
    local_user_id: Option<i64>,
    client_id: Option<&str>,
) -> Result<(), String> {
    let class = classify_inbound_output();
    let now = node.clock().now().unix_timestamp();
    let expires = now + n3ur0n_core::default_ttl_secs(n3ur0n_core::BlobPurpose::Output) as i64;
    let row = BlobInsert {
        hash: blob.hash.clone(),
        size: blob.size as i64,
        mime: blob.mime.clone(),
        expires_at: expires,
        storage_path: path.display().to_string(),
        provenance: "inbound".into(),
        role: "output".into(),
        anchor_kind: "user_session".into(),
        processing_status: "ready".into(),
        local_user_id,
        client_id: client_id.map(String::from),
        conversation_id: None,
        dispatch_id: None,
        capability: None,
        remote_sender_id: None,
        ticket_nonce: None,
        invoke_id: None,
        user_visible: class.user_visible,
        user_deletable: class.user_deletable,
        uploader_id: None,
        recipients_whitelist: None,
    };
    blobs::upsert(node.db(), &row).map_err(|e| e.to_string())
}

/// Upload any local BlobRefs in `args` to `endpoint` before a remote invoke.
pub async fn prepare_invoke_args(
    node: &Node,
    http: &Client,
    endpoint: &str,
    capability: &str,
    args: Value,
) -> Result<Value, String> {
    let refs = collect_blob_refs(&args);
    if refs.is_empty() {
        return Ok(args);
    }
    let recipient = discover_recipient(http, endpoint)
        .await
        .map_err(|e| e.to_string())?;

    for br in &refs {
        let bytes = read_local_bytes(node, &br.hash).ok_or_else(|| {
            format!(
                "blob `{}` not found locally; upload via Files panel first",
                br.hash
            )
        })?;
        upload_blob(
            http,
            node.keypair(),
            endpoint,
            &recipient,
            capability,
            &br.mime,
            &bytes,
        )
        .await
        .map_err(|e| e.to_string())?;

        if let Some(path) = storage_path_for(node, &br.hash) {
            let _ = record_outbound_upload(
                node,
                &br.hash,
                br.size as i64,
                &br.mime,
                &path,
                None,
                None,
            );
        }
    }
    Ok(args)
}

/// Download output BlobRefs from `endpoint` and store locally (class B).
pub async fn fetch_output_blobs(
    node: &Node,
    http: &Client,
    endpoint: &str,
    value: Value,
) -> Result<Value, String> {
    let refs = collect_blob_refs(&value);
    if refs.is_empty() {
        return Ok(value);
    }
    let recipient = discover_recipient(http, endpoint)
        .await
        .map_err(|e| e.to_string())?;

    let mut out = value;
    for br in refs {
        // Skip if already local.
        if read_local_bytes(node, &br.hash).is_some() {
            continue;
        }
        let bytes = download_blob(http, node.keypair(), endpoint, &recipient, &br)
            .await
            .map_err(|e| e.to_string())?;
        if hash_bytes(&bytes) != br.hash {
            return Err(format!("downloaded blob hash mismatch for {}", br.hash));
        }
        let Some(root) = blobs_dir(node) else {
            return Err("blobs_dir not configured".into());
        };
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let path = root.join(&br.hash);
        std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        record_inbound_output(node, &br, &path, None, None)?;
        strip_fetch_url(&mut out, &br.hash);
    }
    Ok(out)
}

fn strip_fetch_url(value: &mut Value, hash: &str) {
    match value {
        Value::Object(map) => {
            if map.get("hash").and_then(|h| h.as_str()) == Some(hash) {
                map.remove("fetch_url");
                return;
            }
            for v in map.values_mut() {
                strip_fetch_url(v, hash);
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                strip_fetch_url(v, hash);
            }
        }
        _ => {}
    }
}

/// Store a user-uploaded file locally (class D).
pub fn store_local_cache(
    node: &Node,
    bytes: &[u8],
    mime: &str,
    local_user_id: Option<i64>,
    client_id: Option<&str>,
) -> Result<BlobRef, String> {
    let hash = hash_bytes(bytes);
    let Some(root) = blobs_dir(node) else {
        return Err("blobs_dir not configured".into());
    };
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let path = root.join(&hash);
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;

    let class = classify_local_cache();
    let now = node.clock().now().unix_timestamp();
    let expires = now + n3ur0n_core::default_ttl_secs(n3ur0n_core::BlobPurpose::Input) as i64;
    let row = BlobInsert {
        hash: hash.clone(),
        size: bytes.len() as i64,
        mime: mime.to_string(),
        expires_at: expires,
        storage_path: path.display().to_string(),
        provenance: "outbound".into(),
        role: "input".into(),
        anchor_kind: "local_cache".into(),
        processing_status: "staged".into(),
        local_user_id,
        client_id: client_id.map(String::from),
        conversation_id: None,
        dispatch_id: None,
        capability: None,
        remote_sender_id: None,
        ticket_nonce: None,
        invoke_id: None,
        user_visible: class.user_visible,
        user_deletable: class.user_deletable,
        uploader_id: None,
        recipients_whitelist: None,
    };
    blobs::upsert(node.db(), &row).map_err(|e| e.to_string())?;

    Ok(BlobRef {
        hash,
        size: bytes.len() as u64,
        mime: mime.to_string(),
        fetch_url: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collects_nested_blob_refs() {
        let v = json!({
            "doc": {"hash": "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08", "size": 5, "mime": "text/plain"},
            "other": 1
        });
        let refs = collect_blob_refs(&v);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].size, 5);
    }
}
