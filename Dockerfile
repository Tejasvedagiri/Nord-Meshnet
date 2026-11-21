# syntax=docker/dockerfile:1

# --- build the init binary (static, musl) ---
FROM rust:1-alpine@sha256:2cccd8b212b37dad15a555589c1b6787da4ab913fe24d8a9b2d99d3498869409 AS init-builder
# hadolint ignore=DL3018
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# musl targets link statically by default, so the binary runs on the glibc
# runtime image with no dependencies (the CI smoke test proves it).
RUN cargo build --release --locked

# --- runtime ---
FROM debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2

# Bumped by reviewed PRs from .github/workflows/upstream.yml, never silently.
ARG NORDVPN_VERSION=6.0.2

SHELL ["/bin/bash", "-o", "pipefail", "-c"]
# hadolint ignore=DL3008
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl gnupg \
 && curl -fsSL https://repo.nordvpn.com/gpg/nordvpn_public.asc \
      | gpg --dearmor -o /usr/share/keyrings/nordvpn.gpg \
 && echo "deb [signed-by=/usr/share/keyrings/nordvpn.gpg] https://repo.nordvpn.com/deb/nordvpn/debian stable main" \
      > /etc/apt/sources.list.d/nordvpn.list \
 && apt-get update \
 && apt-get upgrade -y \
 && apt-get install -y --no-install-recommends \
      "nordvpn=${NORDVPN_VERSION}" e2fsprogs iptables iproute2 nftables procps wireguard-tools \
 && apt-get purge -y curl gnupg \
 && apt-get autoremove -y \
 && rm -rf /var/lib/apt/lists/*

COPY --from=init-builder /src/target/release/nordmesh-init /usr/local/bin/nordmesh-init

# nordvpnd calls `sysctl` by name; route it through the shim (src/sysctl.rs).
RUN real="$(command -v sysctl)" \
 && mv "$real" /usr/sbin/sysctl.real \
 && ln -sf /usr/local/bin/nordmesh-init /usr/sbin/sysctl \
 && if [ "$real" != /usr/sbin/sysctl ]; then ln -sf /usr/local/bin/nordmesh-init "$real"; fi

LABEL org.opencontainers.image.title="nordmesh" \
      org.opencontainers.image.description="NordVPN + Meshnet with peer permissions, configured from environment variables" \
      org.opencontainers.image.licenses="MIT" \
      io.nordmesh.nordvpn-version="${NORDVPN_VERSION}"

VOLUME /var/lib/nordvpn
HEALTHCHECK --interval=60s --timeout=10s --start-period=90s --retries=3 \
  CMD ["/usr/local/bin/nordmesh-init", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/nordmesh-init"]
