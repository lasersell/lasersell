FROM rust:1.86-bookworm AS builder

RUN cargo install lasersell --locked

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/cargo/bin/lasersell /usr/bin/lasersell

ENV HOME=/app
RUN mkdir -p /app/.lasersell
WORKDIR /app/.lasersell

CMD ["lasersell"]
