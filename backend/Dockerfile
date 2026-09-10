FROM rust@sha256:2775a09d208ff0d7c1f50490c45b62db929e87ba1dcbc3f2132ac71a704bcdd3 AS build
WORKDIR /workspace
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY backend/Cargo.toml backend/Cargo.toml
COPY backend/src backend/src
RUN cargo build --locked --release --bin api --bin worker --bin mock_provider

FROM debian@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171
RUN apt-get update \
    && apt-get install --yes --no-install-recommends curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 app
COPY --from=build /workspace/target/release/api /usr/local/bin/api
COPY --from=build /workspace/target/release/worker /usr/local/bin/worker
COPY --from=build /workspace/target/release/mock_provider /usr/local/bin/mock_provider
USER app
