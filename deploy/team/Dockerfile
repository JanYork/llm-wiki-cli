FROM node:24-bookworm-slim AS interface
WORKDIR /build
COPY admin/package*.json ./admin/
RUN npm --prefix admin ci
COPY admin ./admin
COPY web/src/tokens.css web/src/markdown.ts ./web/src/
RUN npm --prefix admin test && npm --prefix admin run build

FROM rust:1.94.1-bookworm AS binary
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends libclang-dev \
    && rm -rf /var/lib/apt/lists/*
ENV CARGO_BUILD_JOBS=2
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --locked --release --bin lwc && cp target/release/lwc /usr/local/bin/lwc

FROM debian:bookworm-slim
LABEL org.opencontainers.image.source="https://github.com/JanYork/llm-wiki-cli"
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --create-home lwc \
    && mkdir -m 0700 /data && chown lwc:lwc /data
COPY --from=binary /usr/local/bin/lwc /usr/local/bin/lwc
COPY --from=interface /build/admin/dist /opt/lwc/admin
USER lwc
WORKDIR /data
EXPOSE 8787
ENTRYPOINT ["lwc"]
CMD ["server", "run", "--config", "/etc/lwc/server.jsonc"]
