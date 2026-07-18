//! Library surface of `n3ur0n-server`. Exposed so integration tests can mount
//! the same router that the binary serves.

// The shared error enum is large; boxing every Result across the crate is a
// wide, low-value change. Deferred perf tuning, not a correctness issue.
#![allow(clippy::result_large_err)]

pub mod auth;
pub mod blob_gc;
pub mod blobs;
pub mod bootstrap;
pub mod bootstrap_config;
pub mod files_api;
pub mod http;
pub mod planner_config;
pub mod settings;
