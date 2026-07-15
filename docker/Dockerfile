# syntax=docker/dockerfile:1.7

# ---------- builder ----------
FROM rust:1.97-bookworm AS builder

WORKDIR /work

# Cache deps separately from sources by first copying manifests only.
# Root Cargo.toml is filtered so the workspace doesn't reference the
# `crates/desktop` member — desktop pulls in Tauri + system GUI deps
# we don't need in the publisher image.
COPY Cargo.toml Cargo.lock ./
RUN sed -i '/"crates\/desktop"/d' Cargo.toml
COPY crates/core/Cargo.toml         crates/core/Cargo.toml
COPY crates/storage/Cargo.toml      crates/storage/Cargo.toml
COPY crates/adapters/Cargo.toml     crates/adapters/Cargo.toml
COPY crates/node/Cargo.toml         crates/node/Cargo.toml
COPY crates/server/Cargo.toml       crates/server/Cargo.toml

# Stub source files to satisfy cargo metadata; real source replaces them next.
RUN mkdir -p crates/core/src crates/storage/src crates/adapters/src \
             crates/node/src  crates/server/src \
 && echo 'fn main() {}' > crates/server/src/main.rs \
 && for d in core storage adapters node; do echo '' > "crates/$d/src/lib.rs"; done

# Pre-cache compile of dependencies. `|| true` because the stub crates have no
# real items; cargo will still resolve and compile every dep, populating the
# cargo registry cache for subsequent layers.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/work/target \
    cargo fetch

# Now copy real sources and build the release binary.
COPY crates ./crates
COPY frontend ./frontend

# Re-strip the desktop member after the real crates/ tree is copied
# (the COPY restored the original tree).
RUN sed -i '/"crates\/desktop"/d' Cargo.toml \
 && rm -rf crates/desktop

# `--bin n3ur0n` builds only the publisher binary; desktop crate is excluded
# at the workspace level so we never pull in Tauri here.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/work/target \
    cargo build --release --bin n3ur0n \
 && cp target/release/n3ur0n /usr/local/bin/n3ur0n

# ---------- runtime ----------
FROM debian:bookworm-slim AS runtime

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl tini \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --system --gid 1000 n3ur0n \
 && useradd  --system --uid 1000 --gid 1000 --home /var/lib/n3ur0n --shell /usr/sbin/nologin n3ur0n \
 && install -d -o n3ur0n -g n3ur0n /var/lib/n3ur0n

COPY --from=builder /usr/local/bin/n3ur0n /usr/local/bin/n3ur0n
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

USER n3ur0n
ENV N3UR0N_CONFIG_DIR=/var/lib/n3ur0n \
    N3UR0N_PORT=4242 \
    RUST_LOG=info,n3ur0n=debug,tower_http=info

EXPOSE 4242
WORKDIR /var/lib/n3ur0n

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/entrypoint.sh"]
