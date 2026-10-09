FROM rust:1.85.1-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
# Build dependencies once in their own layer (cached across source-only changes).
RUN mkdir src && echo 'fn main() {}' > src/main.rs && touch src/lib.rs \
    && cargo build --release --locked && rm -rf src target/release/re0drive* target/release/deps/re0drive* target/release/deps/libre0drive* target/release/.fingerprint/re0drive*
COPY src ./src
COPY web ./web
RUN touch src/main.rs src/lib.rs && cargo build --release --locked && test -x target/release/re0drive

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates tzdata curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home re0drive \
    && mkdir /data && chown re0drive:re0drive /data
COPY --from=build /build/target/release/re0drive /usr/local/bin/re0drive
USER re0drive
ENV RE0DRIVE_BIND=0.0.0.0:8787 RE0DRIVE_DATA=/data TZ=Asia/Hong_Kong
EXPOSE 8787
VOLUME ["/data"]
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s CMD curl -fsS http://127.0.0.1:8787/health || exit 1
ENTRYPOINT ["/usr/local/bin/re0drive"]
