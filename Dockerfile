# Aiven Runtime builds this file from the repository root.
# Runtime credentials belong in platform Secret variables, never build arguments.
FROM rust:1.99.1-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32-unknown-unknown \
    && cargo install --locked --version 0.21.14 trunk \
    && cargo install --locked --version 0.2.129 wasm-bindgen-cli

ENV TRUNK_SKIP_VERSION_CHECK=true \
    TRUNK_TOOLS_WASM_OPT=version_131
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo
COPY crates ./crates
COPY apps ./apps
COPY xtask ./xtask
COPY assets ./assets
COPY contributors ./contributors

RUN cargo build --locked --release -p gridcraft-server
RUN cd apps/gridcraft-web && trunk build --locked --release

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 gridcraft \
    && useradd --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin gridcraft

WORKDIR /app
COPY --from=builder /build/target/release/gridcraft-server /usr/local/bin/gridcraft-server
COPY --from=builder /build/dist/web /app/web

ENV GRIDCRAFT_BIND=0.0.0.0 \
    PORT=8080 \
    GRIDCRAFT_STATIC_DIR=/app/web \
    GRIDCRAFT_MIGRATE_ON_START=false
USER 10001:10001
EXPOSE 8080
ENTRYPOINT ["gridcraft-server"]
CMD ["serve"]
