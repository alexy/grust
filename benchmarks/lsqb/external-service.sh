#!/usr/bin/env bash
# shellcheck disable=SC2034 # Globals are outputs consumed by the sourcing script.
# Qualification helpers for operator-managed Docker services used by the LSQB
# matrix. This file is sourced by run-grust.sh and intentionally does not set
# shell options or traps.

grust_external_prefix() {
    printf '%s\n' "$1" | tr '[:lower:]-' '[:upper:]_'
}

grust_external_endpoint_name() {
    case "$1" in
        sail) printf 'SAIL_ENDPOINT\n' ;;
        postgres-pgq) printf 'POSTGRES_PGQ_URL\n' ;;
        helix) printf 'HELIX_QUERY_URL\n' ;;
        helix-sdk) printf 'HELIX_SDK_BASE_URL\n' ;;
        surreal-sdk) printf 'SURREAL_SDK_URL\n' ;;
        *) return 1 ;;
    esac
}

grust_external_is_enabled() {
    local prefix mode_name
    prefix=$(grust_external_prefix "$1") || return 1
    mode_name="${prefix}_SERVICE_MODE"
    [[ "${!mode_name:-unavailable}" == external ]]
}

# Print the explicit host port from a supported external endpoint without ever
# putting the endpoint itself on a process command line. Qualified services are
# deliberately reachable only through Docker's stable host-gateway name; the
# container attestation below then proves that this exact host port is published
# by the declared container.
grust_external_endpoint_port() {
    local backend=$1 endpoint=$2
    # A same-named exported caller variable must not turn this private copy into
    # process environment inherited by Python.
    export -n endpoint 2>/dev/null || true
    python3 - "$backend" 3<<<"$endpoint" <<'PY'
import os
import shlex
import sys
from typing import Dict, Optional, Set
from urllib.parse import parse_qsl, urlsplit


def fail(message: str) -> "None":
    print(f"external-service.sh: {message}", file=sys.stderr)
    raise SystemExit(1)


backend = sys.argv[1]
try:
    endpoint = os.fdopen(3, encoding="utf-8", errors="strict").read()
except (OSError, UnicodeError):
    fail("external endpoint is not valid UTF-8")

# A Bash here-string contributes one final LF. Preserve and reject any LF that
# was part of the supplied value rather than silently normalizing it.
if endpoint.endswith("\n"):
    endpoint = endpoint[:-1]
if not endpoint or any(ord(character) < 32 or ord(character) == 127 for character in endpoint):
    fail("external endpoint is empty or contains a control character")


def require_host_port(host: Optional[str], port: Optional[int]) -> int:
    if host is None or host.lower() != "host.docker.internal":
        fail("external endpoint host must be host.docker.internal")
    if port is None or not 1 <= port <= 65535:
        fail("external endpoint must contain an explicit TCP port")
    return port


def url_port(value: str, schemes: Set[str]) -> int:
    try:
        parsed = urlsplit(value)
        port = parsed.port
    except ValueError:
        fail("external endpoint is not a valid URL")
    if parsed.scheme.lower() not in schemes or not parsed.netloc:
        fail("external endpoint has an unsupported URL scheme")
    return require_host_port(parsed.hostname, port)


if backend in {"sail", "helix", "helix-sdk"}:
    result = url_port(endpoint, {"http", "https"})
elif backend == "surreal-sdk":
    result = url_port(endpoint, {"ws", "wss"})
elif backend == "postgres-pgq":
    if endpoint.lower().startswith(("postgres://", "postgresql://")):
        try:
            parsed = urlsplit(endpoint)
            overrides = {key.lower() for key, _ in parse_qsl(parsed.query, keep_blank_values=True)}
        except ValueError:
            fail("PostgreSQL external endpoint is not a valid URL")
        if overrides & {"host", "hostaddr", "port"}:
            fail("PostgreSQL URL must not override its network target in query parameters")
        result = url_port(endpoint, {"postgres", "postgresql"})
    else:
        try:
            tokens = shlex.split(endpoint, posix=True)
        except ValueError:
            fail("PostgreSQL external endpoint has invalid quoting")
        values: Dict[str, str] = {}
        for token in tokens:
            key, separator, value = token.partition("=")
            key = key.lower()
            if not separator or not key or key in values:
                fail("PostgreSQL external endpoint must use unique key=value fields")
            values[key] = value
        if "hostaddr" in values:
            fail("PostgreSQL external endpoint must not set hostaddr")
        try:
            port = int(values["port"], 10) if "port" in values else None
        except ValueError:
            fail("PostgreSQL external endpoint port is not an integer")
        result = require_host_port(values.get("host"), port)
else:
    fail("unknown external backend")

print(result)
PY
}

grust_external_load() {
    local backend=$1 prefix mode_name endpoint_name
    local version_name image_name image_id_name container_name worker_name lower_version
    prefix=$(grust_external_prefix "$backend") || return 1
    endpoint_name=$(grust_external_endpoint_name "$backend") || return 1
    mode_name="${prefix}_SERVICE_MODE"
    version_name="${prefix}_VERSION"
    image_name="${prefix}_IMAGE"
    image_id_name="${prefix}_IMAGE_ID"
    container_name="${prefix}_CONTAINER"
    worker_name="${prefix}_WORKER_THREADS"

    GRUST_EXTERNAL_MODE=${!mode_name:-unavailable}
    GRUST_EXTERNAL_ENDPOINT=${!endpoint_name:-}
    GRUST_EXTERNAL_VERSION=${!version_name:-}
    GRUST_EXTERNAL_IMAGE=${!image_name:-}
    GRUST_EXTERNAL_IMAGE_ID=${!image_id_name:-}
    GRUST_EXTERNAL_CONTAINER=${!container_name:-}
    GRUST_EXTERNAL_WORKER_THREADS=${!worker_name:-}
    GRUST_EXTERNAL_ENDPOINT_PORT=

    case "$GRUST_EXTERNAL_MODE" in
        unavailable)
            if [[ -n "$GRUST_EXTERNAL_ENDPOINT" || -n "$GRUST_EXTERNAL_VERSION" || \
                -n "$GRUST_EXTERNAL_IMAGE" || -n "$GRUST_EXTERNAL_IMAGE_ID" || \
                -n "$GRUST_EXTERNAL_CONTAINER" || -n "$GRUST_EXTERNAL_WORKER_THREADS" ]]; then
                echo "external-service.sh: ${backend} identity or endpoint was supplied without ${mode_name}=external" >&2
                return 1
            fi
            GRUST_EXTERNAL_ENABLED=0
            ;;
        external)
            GRUST_EXTERNAL_ENABLED=1
            for required_name in endpoint_name version_name image_name image_id_name container_name; do
                local indirect_name=${!required_name}
                if [[ -z "${!indirect_name:-}" ]]; then
                    echo "external-service.sh: ${backend} external mode requires ${indirect_name}" >&2
                    return 1
                fi
            done
            if [[ ! "$GRUST_EXTERNAL_IMAGE" =~ ^[A-Za-z0-9][^[:space:]]*@sha256:[0-9a-f]{64}$ ]]; then
                echo "external-service.sh: ${image_name} must be pinned to a platform-manifest digest" >&2
                return 1
            fi
            if [[ ! "$GRUST_EXTERNAL_IMAGE_ID" =~ ^sha256:[0-9a-f]{64}$ ]]; then
                echo "external-service.sh: ${image_id_name} must be an image config digest" >&2
                return 1
            fi
            if [[ "$GRUST_EXTERNAL_VERSION" =~ ^[[:space:]]*$ ]]; then
                echo "external-service.sh: ${version_name} must be concrete" >&2
                return 1
            fi
            lower_version=$(printf '%s' "$GRUST_EXTERNAL_VERSION" | tr '[:upper:]' '[:lower:]')
            case "$lower_version" in
                unknown|unreported|unresolved|unspecified|none|n/a)
                    echo "external-service.sh: ${version_name} must be concrete" >&2
                    return 1
                    ;;
            esac
            if [[ -n "$GRUST_EXTERNAL_WORKER_THREADS" && \
                ! "$GRUST_EXTERNAL_WORKER_THREADS" =~ ^[1-9][0-9]*$ ]]; then
                echo "external-service.sh: ${worker_name} must be a positive integer" >&2
                return 1
            fi
            if [[ ! "$GRUST_EXTERNAL_CONTAINER" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]*$ ]]; then
                echo "external-service.sh: ${container_name} must be a Docker name or ID without control characters" >&2
                return 1
            fi
            GRUST_EXTERNAL_ENDPOINT_PORT=$(grust_external_endpoint_port \
                "$backend" "$GRUST_EXTERNAL_ENDPOINT") || {
                echo "external-service.sh: ${endpoint_name} is not bound to an explicit Docker host port" >&2
                return 1
            }
            ;;
        *)
            echo "external-service.sh: ${mode_name} must be unavailable or external" >&2
            return 1
            ;;
    esac
}

grust_external_attest_container() {
    local backend=$1 container=$2 expected_image=$3 expected_image_id=$4 expected_cpus=$5
    local expected_memory=$6 expected_host_port=$7 expected_os=$8 expected_architecture=$9
    local expected_attestation=${10:-} phase=${11:-pre-run} expected_json=null
    local platform_manifest_digest
    local container_inspection image_inspection
    # `local` inherits an existing export attribute in Bash 3.2. Explicitly
    # remove it before these variables hold raw Docker inspection documents,
    # which can contain service credentials.
    export -n container_inspection image_inspection 2>/dev/null || true
    case "$phase" in
        pre-run|post-run) ;;
        *)
            echo "external-service.sh: invalid attestation phase for ${backend}" >&2
            return 1
            ;;
    esac
    if [[ ! "$expected_host_port" =~ ^[1-9][0-9]*$ ]] || \
        (( expected_host_port > 65535 )); then
        echo "external-service.sh: invalid qualified host port for ${backend}" >&2
        return 1
    fi
    if [[ ! "$expected_image" =~ ^[A-Za-z0-9][^[:space:]]*@sha256:[0-9a-f]{64}$ ]]; then
        echo "external-service.sh: invalid pinned image reference for ${backend}" >&2
        return 1
    fi
    if [[ ! "$expected_image_id" =~ ^sha256:[0-9a-f]{64}$ ]]; then
        echo "external-service.sh: invalid image config digest for ${backend}" >&2
        return 1
    fi
    platform_manifest_digest=${expected_image##*@}
    [[ -z "$expected_attestation" ]] || expected_json=$expected_attestation
    container_inspection=$(docker container inspect -- "$container") || {
        echo "external-service.sh: cannot inspect ${backend} container: ${container}" >&2
        return 1
    }
    image_inspection=$(docker image inspect -- "$expected_image") || {
        echo "external-service.sh: cannot inspect ${backend} pinned image" >&2
        return 1
    }
    printf '%s\n%s\n' "$container_inspection" "$image_inspection" | jq -ceSs \
        --argjson expected "$expected_json" \
        --arg backend "$backend" \
        --arg phase "$phase" \
        --arg image_id "$expected_image_id" \
        --arg platform_manifest_digest "$platform_manifest_digest" \
        --arg expected_os "$expected_os" \
        --arg expected_architecture "$expected_architecture" \
        --argjson nano_cpus "$((expected_cpus * 1000000000))" \
        --argjson memory "$expected_memory" \
        --argjson endpoint_port "$expected_host_port" '
        (if length == 2 then .
         else error("external inspection input has an unexpected result count") end
        )
        | .[0] as $containers
        | .[1] as $images
        | ($containers
            | if length == 1 then .[0]
              else error("container inspection returned an unexpected result count") end
        ) as $container
        | ($images
            | if length == 1 then .[0]
              else error("image inspection returned an unexpected result count") end
        ) as $image
        | ([
            ($container.NetworkSettings.Ports // {} | to_entries[]
                | .key as $port_key
                | select($port_key | test("^[1-9][0-9]*/tcp$"))
                | (.value // [])[]
                | select(
                    (.HostPort | type) == "string"
                    and ((.HostPort | tonumber?) == $endpoint_port)
                    and (.HostIp == "0.0.0.0" or .HostIp == "::")
                  )
                | {
                    container_port: ($port_key | split("/")[0] | tonumber),
                    host_ip: .HostIp,
                    host_port: (.HostPort | tonumber),
                    protocol: "tcp"
                  }
            )
          ]
          | sort_by(.container_port, .host_ip, .host_port, .protocol)
          | unique) as $bindings
        | if ($container.Id | type == "string" and test("^[0-9a-f]{64}$")) then .
          else error("container has no immutable 64-hex ID") end
        | if $container.State.Running == true
              and $container.State.Paused == false
              and $container.State.Restarting == false then .
          else error("container is not steadily running") end
        | if (($container.State.StartedAt | type) == "string"
              and ($container.State.StartedAt
                   | test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\\.[0-9]{1,9})?Z$"))
              and ($container.State.StartedAt | startswith("0001-") | not)) then .
          else error("container has no concrete start identity") end
        | if (($container.RestartCount | type) == "number"
              and ($container.RestartCount | floor) == $container.RestartCount
              and $container.RestartCount >= 0) then .
          else error("container has no valid restart count") end
        | if (($image.Id | type) == "string"
              and ($image.Id | test("^sha256:[0-9a-f]{64}$"))) then .
          else error("local image has no immutable runtime ID") end
        | if $container.Image == $image.Id then .
          else error("container image does not match the inspected local image") end
        | if ($image.Id == $image_id or $image.Id == $platform_manifest_digest) then .
          else error("local image ID is neither the registry config nor platform manifest digest") end
        | if $container.Platform == $expected_os
              and $image.Os == $expected_os
              and $image.Architecture == $expected_architecture then .
          else error("container image OS or architecture does not match the benchmark") end
        | if $container.HostConfig.NanoCpus == $nano_cpus then .
          else error("container CPU limit does not match the benchmark limit") end
        | if $container.HostConfig.CpusetCpus == "" then .
          else error("container has a restrictive CPU set") end
        | if $container.HostConfig.Memory == $memory then .
          else error("container memory limit does not match the benchmark limit") end
        | if $container.HostConfig.MemorySwap == $memory then .
          else error("container memory+swap limit does not match the benchmark limit") end
        | if ($bindings | length) > 0 then .
          else error("container does not publish the qualified endpoint port") end
        | {
            architecture: $image.Architecture,
            backend: $backend,
            container_id: $container.Id,
            cpuset_cpus: $container.HostConfig.CpusetCpus,
            endpoint_host: "host.docker.internal",
            endpoint_port: $endpoint_port,
            image_id: $image_id,
            memory_bytes: $container.HostConfig.Memory,
            memory_swap_bytes: $container.HostConfig.MemorySwap,
            nano_cpus: $container.HostConfig.NanoCpus,
            os: $image.Os,
            phase: $phase,
            platform_manifest_digest: $platform_manifest_digest,
            published_bindings: $bindings,
            restart_count: $container.RestartCount,
            runtime_image_id: $image.Id,
            running: $container.State.Running,
            started_at: $container.State.StartedAt
          }
        | . as $attestation
        | if $expected == null
              or (($expected | type) == "object"
                  and (($expected | del(.phase)) == ($attestation | del(.phase)))) then
              $attestation
          else error("container identity or runtime state changed during the benchmark") end
    '
}
