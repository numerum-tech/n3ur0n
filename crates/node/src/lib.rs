//! `n3ur0n-node`: runtime orchestration shared between the server and the
//! desktop shell.
//!
//! This crate owns the things that make a running n3ur0n instance:
//! - persistent identity (loaded from / saved to disk),
//! - SQLite storage handle,
//! - capability registry + backend adapter,
//! - typed handlers for the four protocol verbs.
//!
//! Thin shells (axum HTTP, Tauri IPC) are expected to extract a
//! [`SignedMessage`](n3ur0n_core::SignedMessage), call
//! [`Node::handle`](crate::Node::handle), and re-emit the resulting reply.

pub mod bindings;
pub mod client;
pub mod conversation;
pub mod discovery;
pub mod error;
pub mod handler;
pub mod identity_file;
pub mod manifest;
pub mod node;
pub mod planner;
pub mod registry;
pub mod runtime;

pub use error::{NodeError, NodeResult};
pub use handler::handle_request;
pub use identity_file::IdentityFile;
pub use node::{Node, NodeConfig};
pub use registry::CapabilityRegistry;
