# Multi-stage Dockerfile for the all-in-one TokioNotes gateway.
FROM rust:1.82-slim AS builder
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev ca-certificates && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --release -p tn-gateway

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/tn-gateway /usr/local/bin/tn-gateway
ENV BIND=0.0.0.0:8080
EXPOSE 8080
CMD ["tn-gateway"]

