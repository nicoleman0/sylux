# Linux amd64 beta build; base image indexes are pinned.
FROM node:24.22.2-bookworm-slim@sha256:01d566e8d94d8c619591d78652babfccd3c6992a3dffa9ed5ab39e11cf6c70e9 AS web-build
WORKDIR /src
RUN npm install --global npm@11.19.1
COPY web/package.json web/package-lock.json ./web/
RUN npm --prefix web ci
COPY web/ ./web/
RUN npm --prefix web run check && npm --prefix web run build

FROM rust:1.99.1-bookworm@sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c AS rust-build
WORKDIR /src
RUN apt-get update && apt-get install --yes --no-install-recommends python3 && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock build.rs rust-toolchain.toml ./
COPY src/ ./src/
COPY migrations/ ./migrations/
COPY tests/ ./tests/
COPY tools/ ./tools/
COPY --from=web-build /src/web/dist/ ./web/dist/
# Cache mounts keep rebuilds incremental; the binary is copied out of the cached target.
RUN --mount=type=cache,target=/src/target --mount=type=cache,target=/usr/local/cargo/registry \
    cargo fetch --locked --target x86_64-unknown-linux-gnu \
    && cargo build --locked --release --bin sylux && cp target/release/sylux /sylux \
    && python3 tools/collect-rust-notices.py /rust-notices

FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc AS runtime
RUN apt-get update \
    && apt-get install --yes --no-install-recommends libgcc-s1 \
    && rm -rf /var/lib/apt/lists/*
RUN groupadd --gid 10001 sylux \
    && useradd --uid 10001 --gid sylux --no-create-home --shell /usr/sbin/nologin sylux
COPY --from=rust-build --chown=10001:10001 /sylux /usr/local/bin/sylux
COPY --from=rust-build /rust-notices/ /usr/share/doc/sylux/third-party/
COPY LICENSE /usr/share/doc/sylux/LICENSE
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/sylux"]
CMD ["serve", "--root", "/var/lib/sylux", "--listen", "127.0.0.1:3000", "--tls-cert", "/run/sylux/tls/cert.pem", "--tls-key", "/run/sylux/tls/key.pem"]
