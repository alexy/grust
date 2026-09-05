# syntax=docker/dockerfile:1.7
ARG RUST_IMAGE=rust:1.97.1-trixie@sha256:b1b3c9c0d921d7fa0a6d1f9ec7e4eab87f8c8ec97644c3d791450f131dec813f
ARG RUNTIME_IMAGE=debian:trixie-slim@sha256:d7e12182ce18b85b93007c1dedf31f2d29e01ccf3182cc4017c709b6259bc132
FROM ${RUST_IMAGE} AS build

ARG BENCHMARK_FEATURE=""
ARG CA_CERTIFICATES_BUNDLE_SHA256=714d457d580922dbf1d0be8bd35ba236a842b50b0072ae791582a19adef772a5
ARG LBUG_VERSION=0.20.2
ARG PROTOBUF_DEBIAN_VERSION=3.21.12-11+deb13u1
ARG LSQB_COMMIT=242cb2fd31340ca688954cb94794d74c0d5b6f92
ARG LSQB_TREE=d99fab28d47791dbc0e7173abc4c66d8aadc64ca
ARG LSQB_QUERY_TREE=50937f3d075245e2abd4c00a36c4b3c236766265
ARG LSQB_EXAMPLE_DATA_TREE=45181e6b274d014f8626038e1d398fa1b9e4c19d
RUN git clone --filter=blob:none https://github.com/ldbc/lsqb.git /opt/lsqb \
    && git -C /opt/lsqb checkout --detach "${LSQB_COMMIT}" \
    && test "$(git -C /opt/lsqb rev-parse HEAD^{tree})" = "${LSQB_TREE}" \
    && test "$(git -C /opt/lsqb rev-parse HEAD:cypher)" = "${LSQB_QUERY_TREE}" \
    && test "$(git -C /opt/lsqb rev-parse HEAD:data/social-network-sfexample-projected-fk)" = "${LSQB_EXAMPLE_DATA_TREE}"

# Lance's generated protobuf bindings require protoc at compile time. Pin the
# complete Debian package set so an index update cannot silently select a new
# compiler or runtime library version.
RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        "libprotobuf32t64=${PROTOBUF_DEBIAN_VERSION}" \
        "libprotobuf-dev=${PROTOBUF_DEBIAN_VERSION}" \
        "libprotoc32t64=${PROTOBUF_DEBIAN_VERSION}" \
        "protobuf-compiler=${PROTOBUF_DEBIAN_VERSION}" \
    && test "$(protoc --version)" = "libprotoc 3.21.12" \
    && echo "${CA_CERTIFICATES_BUNDLE_SHA256}  /etc/ssl/certs/ca-certificates.crt" | sha256sum --check --status \
    && apt-get clean \
    && find /var/lib/apt/lists -mindepth 1 -delete

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY benchmarks/lsqb/Cargo.toml benchmarks/lsqb/Cargo.lock ./benchmarks/lsqb/
COPY benchmarks/lsqb/evidence-manifest-v2.json ./benchmarks/lsqb/evidence-manifest-v2.json
COPY benchmarks/lsqb/attacks ./benchmarks/lsqb/attacks
COPY benchmarks/lsqb/src ./benchmarks/lsqb/src
# Ladybug's static archive and the always-linked Turso stack both export SimSIMD.
# GNU ld must accept those duplicate definitions in the Ladybug-only image.
RUN --mount=type=cache,id=grust-lsqb-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=grust-lsqb-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=grust-lsqb-target,target=/src/benchmarks/lsqb/target,sharing=locked \
    case "${BENCHMARK_FEATURE}" in \
        ""|falkor|helix|ladybug|lancedb|neo4j-native|pggraph|postgres-pgq|sail|surreal) ;; \
        *) echo "unsupported benchmark feature: ${BENCHMARK_FEATURE}" >&2; exit 2 ;; \
    esac \
    && if [ "${BENCHMARK_FEATURE}" = ladybug ]; then \
        export RUSTFLAGS='-C link-arg=-Wl,--allow-multiple-definition'; \
    fi \
    && if [ -n "${BENCHMARK_FEATURE}" ]; then \
        cargo build --locked --release \
            --manifest-path benchmarks/lsqb/Cargo.toml \
            --bin grust-lsqb-runner \
            --bin grust-lsqb-matrix \
            --features "${BENCHMARK_FEATURE}"; \
    else \
        cargo build --locked --release \
            --manifest-path benchmarks/lsqb/Cargo.toml \
            --bin grust-lsqb-runner \
            --bin grust-lsqb-matrix; \
    fi \
    && mkdir -p /opt/grust-bin \
    && cp /src/benchmarks/lsqb/target/release/grust-lsqb-runner /opt/grust-bin/ \
    && cp /src/benchmarks/lsqb/target/release/grust-lsqb-matrix /opt/grust-bin/ \
    && if [ "${BENCHMARK_FEATURE}" = neo4j-native ]; then \
        cargo build --locked --release --manifest-path benchmarks/lsqb/Cargo.toml \
            --bin grust-lsqb-neo4j --features neo4j-native \
        && cp /src/benchmarks/lsqb/target/release/grust-lsqb-neo4j /opt/grust-bin/; \
    fi

FROM ${RUNTIME_IMAGE}
ARG GRUST_SOURCE_REVISION=unknown
ARG BENCHMARK_FEATURE=""
ARG CA_CERTIFICATES_BUNDLE_SHA256=714d457d580922dbf1d0be8bd35ba236a842b50b0072ae791582a19adef772a5
ENV GRUST_SOURCE_REVISION=${GRUST_SOURCE_REVISION} \
    BENCHMARK_FEATURE=${BENCHMARK_FEATURE} \
    RUST_VERSION=1.97.1 \
    BENCHMARK_IMAGE=grust-lsqb:local \
    SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt
COPY --from=build /opt/grust-bin/ /usr/local/bin/
COPY --from=build /opt/lsqb /opt/lsqb
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY benchmarks/lsqb/attacks /opt/grust-attacks
RUN mkdir -p /opt/lsqb-mounted \
    && ln -s /opt/lsqb/cypher /opt/lsqb-mounted/cypher \
    && ln -s /opt/lsqb/expected-output /opt/lsqb-mounted/expected-output \
    && ln -s /datasets /opt/lsqb-mounted/data \
    && echo "${CA_CERTIFICATES_BUNDLE_SHA256}  /etc/ssl/certs/ca-certificates.crt" | sha256sum --check --status
LABEL org.opencontainers.image.revision=${GRUST_SOURCE_REVISION} \
      org.opencontainers.image.title="Grust LSQB matrix" \
      io.adversarial.grust.benchmark-feature=${BENCHMARK_FEATURE} \
      io.adversarial.grust.runtime-ca-certificates="20250419; sha256:714d457d580922dbf1d0be8bd35ba236a842b50b0072ae791582a19adef772a5" \
      io.adversarial.grust.build-protoc="3.21.12-11+deb13u1"
ENTRYPOINT ["grust-lsqb-matrix"]
