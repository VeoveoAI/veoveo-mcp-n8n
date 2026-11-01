# syntax=docker/dockerfile:1.27@sha256:4edf897a3ffa55b89f906fc8cc78afdb3f1834cc9c7083565e611a8a7d5fe99e
# The factory environment: the template's Rust toolchain, which builds the server, and
# Node.js with n8n from the VeoveoAI fork, which the server runs as its child. Both are
# Debian trixie (glibc), as the installation's Lore CLI that every start runs is.
#
# n8n comes from the fork's image in the installation's registry: commit 60a31ec8 of
# VeoveoAI/n8n (n8n 2.46.0, the schema check before CREATE SCHEMA,
# N8N_SANDBOX_SERVICE_HOMES, a home for each sandbox, and an MCP client that offers MCP
# 2026-07-28 first), built on node:26.12.1-trixie-slim.
# Only the factory's BuildKit reaches that registry.
FROM registry.veoveo-system.svc.cluster.local:5000/veoveo/n8n@sha256:bc5b5f4c03f43a52b9b58355f00ca7510cdf5ae22184db5eb0d8f5e7d04a9d26 AS n8n

FROM rust:1.99.0-trixie@sha256:6ff07edce8775d0f64be7aba9197229407301bddf2054d62c27b541a6238a181
USER root
# librdkafka1 is the one runtime library of n8n's native modules that the Rust image lacks.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates clang cmake git pkg-config tzdata librdkafka1 \
    && rm -rf /var/lib/apt/lists/* /var/log/apt /var/log/dpkg.log \
    && rustup component add clippy rustfmt
# Node.js and n8n as the fork's image lays them out. Both images are Debian trixie, so
# node and n8n's native modules link against the same glibc and libstdc++.
COPY --from=n8n /usr/local/bin/node /usr/local/bin/node
COPY --from=n8n /usr/local/lib/node_modules/n8n /usr/local/lib/node_modules/n8n
RUN ln -s /usr/local/lib/node_modules/n8n/bin/n8n /usr/local/bin/n8n \
    && node --version && n8n --version
ENV NODE_ENV=production NODE_PATH=/usr/local/lib/node_modules
# The persistent disk keeps fetched crates across restarts; cargo fetches pinned git
# dependencies with git, which the policy admits.
ENV CARGO_HOME=/sandbox/.cargo CARGO_NET_GIT_FETCH_WITH_CLI=true
# OpenShell runs the workload as uid 10001.
RUN groupadd --gid 10001 sandbox \
    && useradd --create-home --uid 10001 --gid 10001 --shell /bin/bash sandbox \
    && chown -R sandbox:sandbox /usr/local/rustup /usr/local/cargo \
    && mkdir -p /sandbox && chown sandbox:sandbox /sandbox
USER sandbox
WORKDIR /sandbox
