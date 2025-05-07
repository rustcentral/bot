ARG RUST_VERSION=1.86.0-stable
ARG APP_NAME=bot

FROM clux/muslrust:${RUST_VERSION} AS chef
USER root
RUN curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash
RUN cargo binstall --no-confirm cargo-chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
ARG APP_NAME
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --target x86_64-unknown-linux-musl --recipe-path recipe.json
COPY . .
RUN cargo build --release --target x86_64-unknown-linux-musl --bin ${APP_NAME}

FROM alpine:3.21 AS final
ARG APP_NAME
ENV APP_NAME=${APP_NAME}

COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/${APP_NAME} /usr/local/bin/

RUN addgroup -S appuser && adduser -S appuser -G appuser
USER appuser

CMD /usr/local/bin/${APP_NAME}
