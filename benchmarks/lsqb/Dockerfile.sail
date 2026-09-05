# Immutable upstream wheel runtime for the Linux ARM64 comparison host.
# This is a Grust-built service image, not an official Sail container release.
FROM python:3.14-slim@sha256:964225e67be639ec050dc6ce66ac0958b67e4ce0603e9847b2fae34cbf23f848
ARG GRUST_SOURCE_REVISION=unknown
RUN test "$(uname -m)" = aarch64 \
    && python -m pip install --no-cache-dir --no-deps \
       'pysail @ https://files.pythonhosted.org/packages/8d/6e/aba6026cc0916d9ed99fad9d584076d6a9652ccef71939b3b933be65ec55/pysail-0.7.1-cp38-abi3-manylinux_2_24_aarch64.whl#sha256=61ffe5d970b2b273c326df1579c7bf262dc4e148013d6d0c592fb7c097ff9ae9' \
    && test "$(sail --version)" = 'sail 0.7.1'
LABEL org.opencontainers.image.source="https://github.com/querygraph/grust" \
      org.opencontainers.image.revision=${GRUST_SOURCE_REVISION} \
      org.opencontainers.image.title="Grust benchmark Sail service" \
      org.opencontainers.image.version="0.7.1" \
      io.adversarial.sail.distribution="pysail-wheel-linux-arm64"
EXPOSE 50051
ENTRYPOINT ["/usr/local/bin/sail"]
CMD ["spark", "server", "--ip", "0.0.0.0", "--port", "50051"]
