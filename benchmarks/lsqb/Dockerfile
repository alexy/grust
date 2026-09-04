# syntax=docker/dockerfile:1.7
ARG RUST_IMAGE=rust:1.97.1-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97
ARG RUNTIME_IMAGE=debian:bookworm-slim@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171
FROM ${RUST_IMAGE} AS build

ARG LSQB_COMMIT=242cb2fd31340ca688954cb94794d74c0d5b6f92
ARG LSQB_TREE=d99fab28d47791dbc0e7173abc4c66d8aadc64ca
ARG LSQB_QUERY_TREE=50937f3d075245e2abd4c00a36c4b3c236766265
ARG LSQB_EXAMPLE_DATA_TREE=45181e6b274d014f8626038e1d398fa1b9e4c19d
RUN git clone --filter=blob:none https://github.com/ldbc/lsqb.git /opt/lsqb \
    && git -C /opt/lsqb checkout --detach "${LSQB_COMMIT}" \
    && test "$(git -C /opt/lsqb rev-parse HEAD^{tree})" = "${LSQB_TREE}" \
    && test "$(git -C /opt/lsqb rev-parse HEAD:cypher)" = "${LSQB_QUERY_TREE}" \
    && test "$(git -C /opt/lsqb rev-parse HEAD:data/social-network-sfexample-projected-fk)" = "${LSQB_EXAMPLE_DATA_TREE}"

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY benchmarks/lsqb/Cargo.toml benchmarks/lsqb/Cargo.lock ./benchmarks/lsqb/
COPY benchmarks/lsqb/src ./benchmarks/lsqb/src
RUN cargo build --locked --release --manifest-path benchmarks/lsqb/Cargo.toml

FROM ${RUNTIME_IMAGE}
ARG GRUST_SOURCE_REVISION=unknown
ENV GRUST_SOURCE_REVISION=${GRUST_SOURCE_REVISION} \
    RUST_VERSION=1.97.1 \
    BENCHMARK_IMAGE=grust-lsqb:local
COPY --from=build /src/benchmarks/lsqb/target/release/grust-lsqb-runner /usr/local/bin/grust-lsqb-runner
COPY --from=build /opt/lsqb /opt/lsqb
COPY benchmarks/lsqb/attacks /opt/grust-attacks
ENTRYPOINT ["grust-lsqb-runner"]
