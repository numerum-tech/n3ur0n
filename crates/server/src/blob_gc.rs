//! Background garbage collection for expired blobs.

use std::path::PathBuf;
use std::time::Duration;

use n3ur0n_node::Node;
use n3ur0n_storage::blobs;
use tracing::info;

const GC_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// Spawn a background task that deletes expired blob index rows and files.
pub fn spawn(node: Node, _blobs_root: PathBuf) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(GC_INTERVAL);
        loop {
            interval.tick().await;
            let now = node.clock().now().unix_timestamp();
            match blobs::delete_expired(node.db(), now) {
                Ok(expired) => {
                    let count = expired.len();
                    for rec in expired {
                        if let Err(e) = std::fs::remove_file(&rec.storage_path) {
                            tracing::debug!(hash = %rec.hash, error = %e, "gc: file already gone");
                        }
                    }
                    if count > 0 {
                        info!(count, "blob gc removed expired blobs");
                    }
                }
                Err(e) => tracing::warn!(error = %e, "blob gc failed"),
            }
        }
    });
}
