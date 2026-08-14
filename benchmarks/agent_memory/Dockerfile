FROM rust:1.94-bookworm AS builder

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY web/dist ./web/dist
COPY skills/using-lwc ./skills/using-lwc
COPY integrations/pi-lwc/extensions/lwc.js ./integrations/pi-lwc/extensions/lwc.js
RUN cargo build --release --locked

FROM python:3.11-slim-bookworm

WORKDIR /app
COPY --from=builder /src/target/release/lwc /usr/local/bin/lwc
COPY benchmarks/agent_memory ./benchmarks/agent_memory
RUN useradd --uid 10001 --create-home --shell /usr/sbin/nologin lwc \
    && mkdir /data \
    && chown lwc:lwc /data

USER lwc
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
EXPOSE 8080
VOLUME ["/data"]
CMD ["python", "-m", "benchmarks.agent_memory.aml_api", "--host", "0.0.0.0", "--port", "8080", "--state-root", "/data"]
