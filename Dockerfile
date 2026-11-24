FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src src
COPY vendor vendor
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends chromium ca-certificates fonts-noto-core fonts-noto-color-emoji fonts-noto-cjk \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/embedgen /usr/local/bin/embedgen
WORKDIR /app
ENV NO_SANDBOX=1 PORT=8080
EXPOSE 8080
CMD ["embedgen", "serve", "/app/embeds.json"]
