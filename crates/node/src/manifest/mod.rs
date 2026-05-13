//! Capability + backend manifest format (cap.toml / backend.toml).
//!
//! v0.3 capabilities are user-authored TOML files loaded at runtime, not
//! Rust `Backend` impls compiled into the binary. This module owns the
//! types and the parser. The watcher + registry integration live next to
//! it (Phase 3/4 wiring; Phase 1 is types + parsing only).
//!
//! See `n3ur0n-capability-manifest-v0.md` for the format spec.

pub mod parser;
pub mod types;

#[cfg(test)]
mod tests;

pub use parser::{
    parse_backend_file, parse_cap_file, load_backend_dir, load_cap_dir,
    ManifestError,
};
pub use types::{
    BackendManifest, BackendKind, CapabilityManifest, BindingSpec,
    OpenAICompatConfig, McpServerConfig, HttpBaseConfig,
    McpTransport, HttpMethod, OutputParser,
};
