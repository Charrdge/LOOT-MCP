# Single binary: loot-mcp (rmcp + libloot). MCP stdio — use `docker run -i`.
FROM rust:1-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
	&& apt-get install -y --no-install-recommends ca-certificates \
	&& rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/loot-mcp /usr/local/bin/loot-mcp
# Session prep cache (libloot Game + plugin headers): reuse within one MCP docker run. Override with LOOT_MCP_CACHE=0.
ENV LOOT_MCP_CACHE=1
ENTRYPOINT ["/usr/local/bin/loot-mcp"]
