FROM ghcr.io/rust-cross/rust-musl-cross:x86_64-musl AS builder
WORKDIR /usr/src/app
COPY Cargo.toml Cargo.lock ./
RUN \
  --mount=type=cache,target=/usr/local/cargo/bin,sharing=locked \
  --mount=type=cache,target=/usr/local/cargo/registry/index,sharing=locked \
  --mount=type=cache,target=/usr/local/cargo/registry/cache,sharing=locked \
  --mount=type=cache,target=/usr/local/cargo/git/db,sharing=locked \
  mkdir src/ && \
  echo "fn main() {}" > src/main.rs && \
  echo "" > src/lib.rs && \
  cargo build --target x86_64-unknown-linux-musl --release --locked && \
  rm -rf src/
COPY . ./
RUN \
  --mount=type=cache,target=/usr/local/cargo/bin,sharing=locked \
  --mount=type=cache,target=/usr/local/cargo/registry/index,sharing=locked \
  --mount=type=cache,target=/usr/local/cargo/registry/cache,sharing=locked \
  --mount=type=cache,target=/usr/local/cargo/git/db,sharing=locked \
  touch src/main.rs src/lib.rs && \
  cargo build --target x86_64-unknown-linux-musl --release --locked

FROM debian:13-slim AS deps
RUN \
  --mount=type=cache,target=/var/cache/apt,sharing=locked \
  --mount=type=cache,target=/var/lib/apt/lists,sharing=locked \
  apt-get update && apt-get install -y ca-certificates

FROM scratch
COPY --from=builder /usr/src/app/target/x86_64-unknown-linux-musl/release/otaflux /otaflux
COPY --from=deps /etc/ssl/certs/ca-certificates.crt /ca-certificates.crt
USER 1000:1000
ENV SSL_CERT_FILE=/ca-certificates.crt
EXPOSE 8080 9090
CMD ["/otaflux"]
