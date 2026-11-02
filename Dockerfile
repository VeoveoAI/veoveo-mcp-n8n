# syntax=docker/dockerfile:1.27@sha256:4edf897a3ffa55b89f906fc8cc78afdb3f1834cc9c7083565e611a8a7d5fe99e
# Stages: dev is the development environment, check is the single gate, build compiles the
# release binary, computer is what a person's Computer adds for n8n, and runtime is the
# deployable image.

ARG COMPONENT=veoveo-mcp-n8n
# n8n from the VeoveoAI fork, commit 8e870075 (n8n 2.46.0, the schema check before
# CREATE SCHEMA, N8N_SANDBOX_SERVICE_HOMES, a home for each sandbox, an MCP client that
# offers MCP 2026-07-28 first, and the environment alone setting the Assistant's
# environment-managed settings, and the Assistant loading MCP skills, asking before any it
# does not trust), built on node:26.12.1-trixie-slim, by digest. The default
# names the installation's registry; a host that reaches it under another address passes
# the same digest there.
ARG N8N_IMAGE=registry.veoveo-system.svc.cluster.local:5000/veoveo/n8n@sha256:c634a5a189a862277382077cddb77bfbe7e237df8f58b995f7de7c17c7b9f1ad

FROM ${N8N_IMAGE} AS n8n

FROM rust:1.99.0-trixie@sha256:6ff07edce8775d0f64be7aba9197229407301bddf2054d62c27b541a6238a181 AS dev
# PostgreSQL 18.6 from the PGDG repository, whose signing key Debian's postgresql-common
# ships. Tests start their own clusters, so the packages create none.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates clang cmake git pkg-config procps postgresql-common \
    && /usr/share/postgresql-common/pgdg/apt.postgresql.org.sh -y \
    && mkdir -p /etc/postgresql-common/createcluster.d \
    && printf 'create_main_cluster = false\n' > /etc/postgresql-common/createcluster.d/veoveo.conf \
    && apt-get install -y --no-install-recommends \
        postgresql-18=18.6-1.pgdg13+2 postgresql-client-18=18.6-1.pgdg13+2 libpq5=18.6-1.pgdg13+2 \
    && rm -rf /var/lib/apt/lists/* /var/log/apt /var/log/dpkg.log \
    && rustup component add clippy rustfmt
ENV PATH=/usr/lib/postgresql/18/bin:${PATH}
# Node.js and n8n as the fork's image lays them out, for the tests that run a real n8n.
# Both images are Debian trixie; librdkafka1 is the one library of n8n's native modules
# the Rust image lacks.
RUN apt-get update \
    && apt-get install -y --no-install-recommends librdkafka1 \
    && rm -rf /var/lib/apt/lists/* /var/log/apt /var/log/dpkg.log
COPY --from=n8n /usr/local/bin/node /usr/local/bin/node
COPY --from=n8n /usr/local/lib/node_modules/n8n /usr/local/lib/node_modules/n8n
RUN ln -s /usr/local/lib/node_modules/n8n/bin/n8n /usr/local/bin/n8n && n8n --version
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    cargo install --locked \
        cargo-nextest@0.9.148 \
        cargo-deny@0.20.2 \
        cargo-hack@0.6.45 \
        cargo-semver-checks@0.51.0 \
        cargo-insta@1.49.0 \
    && cargo install --locked sqlx-cli@0.9.0 --no-default-features --features postgres,rustls,sqlx-toml
COPY env/profile.d/rust.sh /etc/profile.d/rust.sh
# A development environment installs toolchains, components and crates, so the sandbox user owns them.
RUN useradd --create-home --uid 1000 --shell /bin/bash sandbox \
    && chown -R sandbox:sandbox /usr/local/rustup /usr/local/cargo
USER sandbox

FROM dev AS source
WORKDIR /home/sandbox/src
COPY --chown=sandbox:sandbox . .

FROM source AS check
# The check and build steps keep Cargo's registry, its Git checkouts and the workspace
# target in BuildKit cache mounts of the build host, so consecutive checks reuse compiled
# crates. They run as the sandbox user, so these caches are its own, apart from the
# root-owned registry cache of the dev stage's cargo install. Factory builds never use them.
# The committed .sqlx query metadata must match the crate's queries against the kernel
# migrations of the pinned veoveo-core checkout and the crate's own migrations, applied
# in the server's schema. The Git cache may hold other core checkouts, so the store is
# the revision Cargo.lock pins.
RUN --mount=type=cache,id=veoveo-mcp-n8n-cargo-registry,target=/usr/local/cargo/registry,uid=1000,sharing=locked \
    --mount=type=cache,id=veoveo-mcp-n8n-cargo-git,target=/usr/local/cargo/git,uid=1000,sharing=locked \
    --mount=type=cache,id=veoveo-mcp-n8n-target,target=/home/sandbox/src/target,uid=1000,sharing=locked \
    cargo fetch --locked \
    && core="$(sed -n 's|^source = "git+https://github.com/VeoveoAI/veoveo-core?rev=[0-9a-f]*#\([0-9a-f]\{7\}\)[0-9a-f]*"$|\1|p' Cargo.lock | sort -u)" \
    && test "$(echo "${core}" | wc -w)" -eq 1 \
    && store="$(echo /usr/local/cargo/git/checkouts/veoveo-core-*/"${core}"/platform/store)" \
    && test -d "${store}" \
    && initdb --pgdata /tmp/sqlx/data --username postgres --auth=trust --no-instructions --no-sync \
    && pg_ctl start --pgdata /tmp/sqlx/data --wait --silent \
        --options "-c listen_addresses=127.0.0.1 -c unix_socket_directories=/tmp/sqlx -c fsync=off" \
    && createdb --host 127.0.0.1 --username postgres veoveo \
    && DATABASE_URL=postgres://postgres@127.0.0.1/veoveo cargo sqlx migrate run \
        --source "${store}/migrations" --config "${store}/sqlx.toml" \
    && psql --host 127.0.0.1 --username postgres --dbname veoveo --quiet \
        --command "SELECT kernel.ensure_server('n8n')" \
    && DATABASE_URL=postgres://postgres@127.0.0.1/veoveo cargo sqlx migrate run \
        --source crates/veoveo-mcp-n8n/migrations \
        --config crates/veoveo-mcp-n8n/sqlx.toml \
    && DATABASE_URL=postgres://postgres@127.0.0.1/veoveo cargo sqlx prepare --workspace --check \
        -- --all-targets --all-features \
    && pg_ctl stop --pgdata /tmp/sqlx/data --mode fast --wait --silent \
    && rm -rf /tmp/sqlx
RUN --mount=type=cache,id=veoveo-mcp-n8n-cargo-registry,target=/usr/local/cargo/registry,uid=1000,sharing=locked \
    --mount=type=cache,id=veoveo-mcp-n8n-cargo-git,target=/usr/local/cargo/git,uid=1000,sharing=locked \
    --mount=type=cache,id=veoveo-mcp-n8n-target,target=/home/sandbox/src/target,uid=1000,sharing=locked \
    cargo fmt --check \
    && cargo clippy --locked --all-targets --all-features -- -D warnings \
    && cargo deny --locked check \
    && cargo hack check --each-feature --no-dev-deps \
    && cargo nextest run --locked --profile ci

FROM source AS build
ARG COMPONENT
# target/ is a cache mount, so the binary leaves it in the same step.
RUN --mount=type=cache,id=veoveo-mcp-n8n-cargo-registry,target=/usr/local/cargo/registry,uid=1000,sharing=locked \
    --mount=type=cache,id=veoveo-mcp-n8n-cargo-git,target=/usr/local/cargo/git,uid=1000,sharing=locked \
    --mount=type=cache,id=veoveo-mcp-n8n-target,target=/home/sandbox/src/target,uid=1000,sharing=locked \
    cargo build --locked --release --bin "${COMPONENT}" \
    && cp "target/release/${COMPONENT}" "/home/sandbox/${COMPONENT}"

FROM source AS computer-build
# target/ is a cache mount, so the binary leaves it in the same step.
RUN --mount=type=cache,id=veoveo-mcp-n8n-cargo-registry,target=/usr/local/cargo/registry,uid=1000,sharing=locked \
    --mount=type=cache,id=veoveo-mcp-n8n-cargo-git,target=/usr/local/cargo/git,uid=1000,sharing=locked \
    --mount=type=cache,id=veoveo-mcp-n8n-target,target=/home/sandbox/src/target,uid=1000,sharing=locked \
    cargo build --locked --release --bin veoveo-n8n-computer \
    && cp target/release/veoveo-n8n-computer /home/sandbox/veoveo-n8n-computer

# What a person's Computer runs for n8n, as its user, uid 10001, reads it: every file
# readable and every directory traversable by everyone, root owning them, whatever modes
# the build context and the fork's image give them. The check runs as that user: it
# reads every tree, lists the Veoveo nodes and runs Node.js, npm, n8n and the program.
FROM n8n AS computer-files
USER root
COPY --from=computer-build /home/sandbox/veoveo-n8n-computer /usr/local/bin/veoveo-n8n-computer
COPY nodes /opt/veoveo/n8n/nodes
RUN chmod -R a+rX /usr/local/lib/node_modules/npm /usr/local/lib/node_modules/n8n /opt/veoveo/n8n \
    && chmod 0755 /usr/local/bin/node /usr/local/bin/veoveo-n8n-computer
# The fork image's WORKDIR, /home/node, is closed to uid 10001; the check starts from /.
WORKDIR /
USER 10001
RUN set -e; scratch="$(mktemp -d)"; export HOME="$scratch"; \
    unreadable="$(find /usr/local/lib/node_modules/npm /usr/local/lib/node_modules/n8n /opt/veoveo/n8n \
        \( -type d \( ! -readable -o ! -executable \) \) -o \( -type f ! -readable \) 2>&1 | head -5)"; \
    if [ -n "$unreadable" ]; then echo "uid 10001 cannot read: $unreadable" >&2; exit 1; fi; \
    ls /opt/veoveo/n8n/nodes/n8n-nodes-veoveo; \
    node --version; npm --version; /usr/local/lib/node_modules/n8n/bin/n8n --version; \
    veoveo-n8n-computer --help >/dev/null; \
    rm -rf "$scratch"

# What a person's Computer adds for n8n: Node.js, npm and n8n as the fork's image lays them
# out, the Veoveo nodes and veoveo-n8n-computer, with the modes above. npm installs the
# Assistant's workspace packages in its sandboxes. An installation copies these paths into
# its Computer image, a Debian trixie image as the fork's and the build's are, so node,
# n8n's native modules and the binary find the same C library and libstdc++ there; n8n's
# native modules also need librdkafka1, which the Computer image installs.
FROM scratch AS computer
COPY --from=computer-files /usr/local/bin/node /usr/local/bin/node
COPY --from=computer-files /usr/local/lib/node_modules/npm /usr/local/lib/node_modules/npm
COPY --from=computer-files /usr/local/lib/node_modules/n8n /usr/local/lib/node_modules/n8n
COPY --from=computer-files /usr/local/bin/veoveo-n8n-computer /usr/local/bin/veoveo-n8n-computer
COPY --from=computer-files /opt/veoveo/n8n /opt/veoveo/n8n

# The runtime is the fork's n8n image with the server beside it: the server starts n8n as
# its child. Both are Debian trixie builds, so the binary's glibc is the image's.
FROM n8n AS runtime
ARG COMPONENT
ARG VERSION
ARG REVISION
USER root
# The registration key and n8n's state persist under /var/lib/veoveo/n8n; mount a volume
# there to keep the server's namespace and n8n's keys across container restarts.
RUN useradd --system --uid 10001 --no-create-home "${COMPONENT}" \
    && mkdir -p /var/lib/veoveo/n8n/keys /var/lib/veoveo/n8n/state /var/lib/veoveo/n8n/files \
    && chown -R "${COMPONENT}:${COMPONENT}" /var/lib/veoveo/n8n
COPY --from=build "/home/sandbox/${COMPONENT}" "/usr/local/bin/${COMPONENT}"
COPY n8n /opt/veoveo/n8n/n8n
COPY nodes /opt/veoveo/n8n/nodes
COPY crates/veoveo-mcp-n8n/assets /opt/veoveo/n8n/crates/veoveo-mcp-n8n/assets
WORKDIR /opt/veoveo/n8n
ENV VEOVEO_N8N_STATE=/var/lib/veoveo/n8n/state VEOVEO_N8N_FILES=/var/lib/veoveo/n8n/files
USER ${COMPONENT}
EXPOSE 8811
# The exec form cannot expand build arguments, so the binary path is fixed here.
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/veoveo-mcp-n8n"]
LABEL org.opencontainers.image.title="${COMPONENT}" \
      org.opencontainers.image.source="https://github.com/VeoveoAI/${COMPONENT}" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}" \
      org.opencontainers.image.licenses="Apache-2.0"
