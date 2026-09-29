ARG BUILDER_IMAGE=rust:1.98.1-slim-bookworm
ARG RUNTIME_IMAGE=debian:bookworm-slim

FROM scratch AS inputs
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY LICENSE ./LICENSE
COPY config.example.toml ./config.example.toml
COPY proto ./proto
COPY client ./client
COPY server ./server
COPY docs ./docs

FROM ${BUILDER_IMAGE} AS chef
ARG CARGO_CHEF_VERSION=0.1.78
RUN cargo install cargo-chef --version "${CARGO_CHEF_VERSION}" --locked
WORKDIR /src

FROM chef AS planner
COPY --from=inputs / /src
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY --from=planner /src/recipe.json recipe.json
# Keep dependencies in an exportable layer, not ephemeral BuildKit cache mounts.
RUN cargo chef cook --locked --release --package moenotes-server --recipe-path recipe.json
COPY --from=inputs / /src
RUN cargo build --locked --release -p moenotes-server

FROM ${RUNTIME_IMAGE}
ARG VERSION=unknown
ARG REVISION=unknown
ARG SOURCE=https://github.com/StarMoe-org/moenotes-api
LABEL org.opencontainers.image.title="moenotes-api" \
      org.opencontainers.image.description="Our Notes API client and HTTP query gateway" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}" \
      org.opencontainers.image.source="${SOURCE}"
COPY --from=build /src/target/release/moenotes-server /usr/local/bin/moenotes-server
COPY --from=inputs /LICENSE /usr/share/doc/moenotes-api/LICENSE
COPY --from=inputs /proto/NOTICE.md /usr/share/doc/moenotes-api/PROTOCOL-NOTICE.md
COPY --from=inputs /config.example.toml /usr/share/doc/moenotes-api/config.example.toml
RUN mkdir -p /var/lib/moenotes && chown 65532:65532 /var/lib/moenotes && chmod 700 /var/lib/moenotes
USER 65532:65532
# Only used when configuration is missing; configured listen settings are unchanged.
ENV MOENOTES_BOOTSTRAP_LISTEN=0.0.0.0:8080
ENV MOENOTES_DEFAULT_CONFIG=/var/lib/moenotes/config.toml
EXPOSE 8080
ENTRYPOINT ["moenotes-server"]
CMD ["serve"]
