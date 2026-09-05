#!/usr/bin/env bash
# shellcheck disable=SC2034 # Qualification variables are consumed by indirection.
# SDK qualification identities must not borrow direct-HTTP environment values.
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
# shellcheck source=benchmarks/lsqb/external-service.sh
source "${root}/external-service.sh"

[[ $(grust_external_endpoint_name helix) == HELIX_QUERY_URL ]]
[[ $(grust_external_endpoint_name helix-sdk) == HELIX_SDK_BASE_URL ]]
[[ $(grust_external_endpoint_name surreal-sdk) == SURREAL_SDK_URL ]]
[[ $(grust_external_prefix helix-sdk) == HELIX_SDK ]]
[[ $(grust_external_prefix surreal-sdk) == SURREAL_SDK ]]
[[ $(grust_external_endpoint_port helix-sdk http://host.docker.internal:18082) == 18082 ]]
[[ $(grust_external_endpoint_port surreal-sdk ws://host.docker.internal:18083) == 18083 ]]

for endpoint in http://host.docker.internal:18083 ws://example.invalid:18083 ws://host.docker.internal; do
    if grust_external_endpoint_port surreal-sdk "$endpoint" >/dev/null 2>&1; then
        echo 'SDK qualification accepted an invalid endpoint' >&2
        exit 1
    fi
done
if grust_external_endpoint_port helix-sdk ws://host.docker.internal:18082 >/dev/null 2>&1; then
    echo 'Helix SDK qualification accepted a non-HTTP endpoint' >&2
    exit 1
fi

unset HELIX_SDK_SERVICE_MODE HELIX_SDK_BASE_URL HELIX_SDK_VERSION HELIX_SDK_IMAGE
unset HELIX_SDK_IMAGE_ID HELIX_SDK_CONTAINER HELIX_SDK_WORKER_THREADS
HELIX_QUERY_URL=http://host.docker.internal:18081/v1/query
HELIX_SERVICE_MODE=external
grust_external_load helix-sdk
[[ "$GRUST_EXTERNAL_ENABLED" == 0 ]]
[[ -z "$GRUST_EXTERNAL_ENDPOINT" ]]
HELIX_SDK_BASE_URL=http://host.docker.internal:18082
if grust_external_load helix-sdk >/dev/null 2>&1; then
    echo 'SDK endpoint without independent qualification was accepted' >&2
    exit 1
fi
printf 'SDK endpoint identity and protocol checks passed\n'
