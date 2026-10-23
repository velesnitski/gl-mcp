FROM rust:1.94-slim AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/gl-mcp /usr/local/bin/gl-mcp
EXPOSE 8000
ENTRYPOINT ["gl-mcp"]
# Inside a container the listener must leave loopback, so GL_MCP_HTTP_TOKEN is
# required: without it the server refuses to start.
CMD ["--transport", "http", "--host", "0.0.0.0", "--port", "8000"]
