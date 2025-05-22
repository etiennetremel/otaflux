FROM ghcr.io/rust-cross/rust-musl-cross:x86_64-musl AS builder
WORKDIR /usr/src/app
# Cache Rust dependencies to speed-up build time
COPY Cargo.toml Cargo.lock ./
RUN mkdir src/ && echo "fn main() {}" > src/main.rs && \
    cargo build --target x86_64-unknown-linux-musl --release --locked && \
    rm -rf src/
# Build project binary
COPY . ./
RUN cargo build --target x86_64-unknown-linux-musl --release --locked

FROM debian:12-slim AS user
RUN useradd -u 1000 -U -m -s /bin/false otaflux

FROM scratch
COPY --from=builder /usr/src/app/target/x86_64-unknown-linux-musl/release/otaflux /otaflux
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /ca-certificates.crt
COPY --from=user /etc/passwd /etc/passwd
COPY --from=user /etc/group /etc/group
USER otaflux
ENV SSL_CERT_FILE=/ca-certificates.crt
EXPOSE 8080
CMD ["/otaflux"]
