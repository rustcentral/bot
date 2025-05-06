FROM docker.io/rust:latest AS builder

RUN apt-get update \
 && apt-get install -y --no-install-recommends sccache
ENV RUSTC_WRAPPER=sccache SCCACHE_DIR=/sccache

WORKDIR /build
COPY ./ ./

RUN --mount=type=cache,target=/usr/local/cargo/registry \
  --mount=type=cache,target=$SCCACHE_DIR,sharing=locked \
  cargo build --release --bin bot && sccache --show-stats


FROM debian:bookworm-slim

COPY --from=builder /build/target/release/bot /bin/bot

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates

RUN update-ca-certificates

CMD ["/bin/bot"]
