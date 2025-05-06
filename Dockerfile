FROM ghcr.io/rust-cross/rust-musl-cross:x86_64-musl AS builder
WORKDIR /usr/src/app
COPY . ./
RUN cargo build --target x86_64-unknown-linux-musl --release --locked

FROM scratch
COPY --from=builder /usr/src/app/target/x86_64-unknown-linux-musl/release/otaflux /otaflux
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /ca-certificates.crt
ENV SSL_CERT_FILE=/ca-certificates.crt
EXPOSE 8080
CMD ["/otaflux"]
