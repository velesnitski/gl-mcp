FROM rust:1.94-slim AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/gl-mcp /usr/local/bin/gl-mcp
EXPOSE 8000
ENTRYPOINT ["gl-mcp"]
CMD ["--transport", "http", "--port", "8000"]
