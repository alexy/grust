#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

PGGRAPH_CONNECTION_STRING_WAS_SET=0
if [[ -n "${PGGRAPH_CONNECTION_STRING+x}" ]]; then
  PGGRAPH_CONNECTION_STRING_WAS_SET=1
fi

CONFIG="${GRUST_INTEGRATION_CONFIG:-integration/backends.conf}"
if [[ -f "$CONFIG" ]]; then
  # shellcheck disable=SC1090
  source "$CONFIG"
fi

BACKENDS=()
NO_START=0
KEEP_RUNNING=0
MODE="${GRUST_INTEGRATION_MODE:-auto}"
PROFILE="${GRUST_INTEGRATION_PROFILE:-}"

ALL_BACKENDS=(sail surreal falkor helix ladybug lancedb cocoindex pggraph)
DOCKER_BACKENDS=(surreal falkor ladybug lancedb cocoindex pggraph)
QUICK_BACKENDS=(ladybug lancedb cocoindex)

usage() {
  cat <<'USAGE'
Usage:
  scripts/integration-test.sh [--profile NAME] [--mode MODE] [--backend NAME ...] [--no-start] [--keep-running]
  scripts/integration-test.sh doctor [--profile NAME] [--mode MODE]

Starts configured local backend services when needed, then runs live backend
tests. A live test fails if its backend is absent; no successful run is produced
by silently skipping an unavailable service.

Backends: sail, surreal, falkor, helix, ladybug, lancedb, cocoindex, pggraph

Profiles:
  quick   Local integration checks that do not need daemons: ladybug, lancedb, cocoindex
  docker  New-user path: Docker-backed services plus local checks
  all     Full maintainer matrix, including source/manual-only backends

Modes:
  auto    Prefer an already-running service, then configured source, then Docker
  docker  Use Docker Compose for Docker-backed services; do not use source checkouts
  source  Use configured source checkouts; do not start Docker services

Config: integration/backends.conf
USAGE
}

profile_backends() {
  case "$1" in
    quick)
      printf '%s\n' "${QUICK_BACKENDS[@]}"
      ;;
    docker)
      printf '%s\n' "${DOCKER_BACKENDS[@]}"
      ;;
    all)
      printf '%s\n' "${ALL_BACKENDS[@]}"
      ;;
    *)
      echo "unknown profile: $1" >&2
      exit 2
      ;;
  esac
}

DOCTOR=0
if [[ "${1:-}" == "doctor" ]]; then
  DOCTOR=1
  shift
fi

while [[ $# -gt 0 ]]; do
  case "$1" in
    --backend)
      BACKENDS+=("${2:?missing backend name}")
      shift 2
      ;;
    --mode)
      MODE="${2:?missing mode name}"
      shift 2
      ;;
    --profile)
      PROFILE="${2:?missing profile name}"
      shift 2
      ;;
    --no-start)
      NO_START=1
      shift
      ;;
    --keep-running)
      KEEP_RUNNING=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "$MODE" in
  auto|docker|source) ;;
  *)
    echo "unknown mode: $MODE" >&2
    usage >&2
    exit 2
    ;;
esac

if [[ ${#BACKENDS[@]} -eq 0 ]]; then
  if [[ -n "$PROFILE" ]]; then
    # shellcheck disable=SC2207
    BACKENDS=($(profile_backends "$PROFILE"))
  else
    # shellcheck disable=SC2206
    BACKENDS=(${GRUST_INTEGRATION_BACKENDS:-sail surreal falkor helix ladybug lancedb cocoindex pggraph})
  fi
fi

STATE_DIR="${TMPDIR:-/tmp}/grust-integration"
mkdir -p "$STATE_DIR"
PIDS=()
COMPOSE_SERVICES=()

cleanup() {
  if [[ "$KEEP_RUNNING" -eq 1 ]]; then
    return
  fi
  for pid in "${PIDS[@]:-}"; do
    if kill -0 "$pid" >/dev/null 2>&1; then
      kill "$pid" >/dev/null 2>&1 || true
    fi
  done
  if [[ ${#COMPOSE_SERVICES[@]} -gt 0 ]]; then
    docker compose -f docker-compose.integration.yml stop "${COMPOSE_SERVICES[@]}" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

port_open() {
  local host="$1"
  local port="$2"
  bash -c ":</dev/tcp/${host}/${port}" >/dev/null 2>&1
}

wait_port() {
  local name="$1"
  local host="$2"
  local port="$3"
  local deadline=$((SECONDS + 120))
  until port_open "$host" "$port"; do
    if (( SECONDS >= deadline )); then
      echo "timed out waiting for $name on $host:$port" >&2
      return 1
    fi
    sleep 1
  done
}

wait_pggraph() {
  local deadline=$((SECONDS + 120))
  until pg_isready -h "$PGGRAPH_HOST" -p "$PGGRAPH_PORT" -U "${PGGRAPH_USER:-postgres}" -d "${PGGRAPH_DB:-graph}" >/dev/null 2>&1; do
    if (( SECONDS >= deadline )); then
      echo "timed out waiting for pggraph PostgreSQL readiness on $PGGRAPH_HOST:$PGGRAPH_PORT" >&2
      return 1
    fi
    sleep 1
  done
}

pggraph_has_graph_extension() {
  if ! command -v psql >/dev/null 2>&1; then
    return 1
  fi
  local result
  result="$(PGPASSWORD="${PGGRAPH_PASSWORD:-postgres}" psql "$PGGRAPH_CONNECTION_STRING" -Atqc "SELECT EXISTS (SELECT 1 FROM pg_available_extensions WHERE name = 'graph')" 2>/dev/null | tr -d '[:space:]')" || return 1
  [[ "$result" == "t" ]]
}

first_free_port() {
  local port="$1"
  while port_open "$PGGRAPH_HOST" "$port"; do
    port=$((port + 1))
  done
  echo "$port"
}

set_pggraph_port() {
  PGGRAPH_PORT="$1"
  PGGRAPH_CONNECTION_STRING="host=${PGGRAPH_HOST} port=${PGGRAPH_PORT} user=${PGGRAPH_USER:-postgres} password=${PGGRAPH_PASSWORD:-postgres} dbname=${PGGRAPH_DB:-graph}"
  export PGGRAPH_PORT PGGRAPH_CONNECTION_STRING
}

start_process() {
  local name="$1"
  local dir="$2"
  shift 2
  echo "starting $name from $dir: $*"
  (
    cd "$dir"
    exec "$@"
  ) >"$STATE_DIR/$name.log" 2>&1 &
  PIDS+=("$!")
}

start_compose() {
  local name="$1"
  echo "starting $name with docker compose"
  docker compose -f docker-compose.integration.yml up -d "$name"
  COMPOSE_SERVICES+=("$name")
}

can_use_source() {
  [[ "$MODE" != "docker" ]]
}

can_use_docker() {
  [[ "$MODE" != "source" ]]
}

start_sail() {
  if port_open "$SAIL_HOST" "$SAIL_PORT"; then
    echo "sail already listening on $SAIL_HOST:$SAIL_PORT"
    return
  fi
  [[ "$NO_START" -eq 0 ]] || return
  if can_use_source && [[ -n "${SAIL_SOURCE:-}" && -d "$SAIL_SOURCE" ]]; then
    if [[ -x "$SAIL_SOURCE/target/release/sail" ]]; then
      start_process sail "$SAIL_SOURCE" "$SAIL_SOURCE/target/release/sail" spark server --port "$SAIL_PORT"
    elif command -v sail >/dev/null 2>&1; then
      start_process sail "$SAIL_SOURCE" sail spark server --port "$SAIL_PORT"
    elif command -v hatch >/dev/null 2>&1; then
      start_process sail "$SAIL_SOURCE" hatch run sail spark server --port "$SAIL_PORT"
    else
      echo "no Sail launcher found; build / install Sail or start it manually" >&2
    fi
  elif [[ "$MODE" == "docker" ]]; then
    echo "sail has no configured Docker Compose service; use --mode auto/source with SAIL_SOURCE or start Sail manually" >&2
    return 1
  fi
}

start_surreal() {
  if port_open "$SURREAL_HOST" "$SURREAL_PORT"; then
    echo "surreal already listening on $SURREAL_HOST:$SURREAL_PORT"
    return
  fi
  [[ "$NO_START" -eq 0 ]] || return
  if can_use_source && [[ -n "${SURREAL_SOURCE:-}" && -d "$SURREAL_SOURCE" ]]; then
    start_process surreal "$SURREAL_SOURCE" cargo run --no-default-features --features storage-mem,http,scripting -- start --log info --user root --pass root memory
  elif can_use_docker && command -v docker >/dev/null 2>&1; then
    start_compose surreal
  fi
}

start_falkor() {
  if port_open "$FALKOR_HOST" "$FALKOR_PORT"; then
    echo "falkor already listening on $FALKOR_HOST:$FALKOR_PORT"
    return
  fi
  [[ "$NO_START" -eq 0 ]] || return
  if can_use_source && [[ -n "${FALKOR_SOURCE:-}" && -d "$FALKOR_SOURCE" && -f "$FALKOR_SOURCE/Makefile" && -x "$(command -v redis-server || true)" ]]; then
    start_process falkor "$FALKOR_SOURCE" make run
  elif can_use_docker && command -v docker >/dev/null 2>&1; then
    start_compose falkor
  fi
}

start_helix() {
  if port_open "$HELIX_HOST" "$HELIX_PORT"; then
    echo "helix already listening on $HELIX_HOST:$HELIX_PORT"
    return
  fi
  [[ "$NO_START" -eq 0 ]] || return
  if can_use_source && [[ -n "${HELIX_SOURCE:-}" && -d "$HELIX_SOURCE" ]]; then
    local project_dir="${HELIX_PROJECT_DIR:-$STATE_DIR/helix-project}"
    if [[ -x "$HELIX_SOURCE/target/release/helix" ]]; then
      mkdir -p "$project_dir"
      if [[ ! -f "$project_dir/helix.toml" ]]; then
        "$HELIX_SOURCE/target/release/helix" init --path "$project_dir" --quiet local >"$STATE_DIR/helix-init.log" 2>&1
      fi
      start_process helix "$project_dir" "$HELIX_SOURCE/target/release/helix" run dev --foreground --port "$HELIX_PORT"
    elif command -v helix >/dev/null 2>&1; then
      mkdir -p "$project_dir"
      if [[ ! -f "$project_dir/helix.toml" ]]; then
        helix init --path "$project_dir" --quiet local >"$STATE_DIR/helix-init.log" 2>&1
      fi
      start_process helix "$project_dir" helix run dev --foreground --port "$HELIX_PORT"
    else
      echo "no Helix launcher found; build / install Helix or start it manually" >&2
    fi
  elif [[ "$MODE" == "docker" ]]; then
    echo "helix has no configured Docker Compose service; use --mode auto/source with HELIX_SOURCE or start Helix manually" >&2
    return 1
  fi
}

start_pggraph() {
  if port_open "$PGGRAPH_HOST" "$PGGRAPH_PORT"; then
    if pggraph_has_graph_extension; then
      echo "pggraph/postgres already listening with graph extension on $PGGRAPH_HOST:$PGGRAPH_PORT"
      return
    fi
    echo "postgres is listening on $PGGRAPH_HOST:$PGGRAPH_PORT but the graph extension is unavailable"
    if [[ "$NO_START" -eq 1 || "$MODE" == "source" ]]; then
      return 1
    fi
    if can_use_docker && [[ -n "${PGGRAPH_IMAGE:-}" ]] && command -v docker >/dev/null 2>&1; then
      local requested_port="$PGGRAPH_PORT"
      local fallback_port
      fallback_port="$(first_free_port 55432)"
      set_pggraph_port "$fallback_port"
      if [[ "$PGGRAPH_CONNECTION_STRING_WAS_SET" -eq 1 ]]; then
        echo "overriding explicit PGGRAPH_CONNECTION_STRING for Docker pgGraph fallback on $PGGRAPH_HOST:$PGGRAPH_PORT"
      else
        echo "starting Docker pgGraph fallback on $PGGRAPH_HOST:$PGGRAPH_PORT instead of occupied port $requested_port"
      fi
      start_compose pggraph
      return
    fi
    return 1
  fi
  [[ "$NO_START" -eq 0 ]] || return
  if can_use_docker && [[ -n "${PGGRAPH_IMAGE:-}" ]] && command -v docker >/dev/null 2>&1; then
    start_compose pggraph
  else
    echo "pggraph startup is not configured yet; start PostgreSQL with the graph extension manually or set PGGRAPH_IMAGE" >&2
  fi
}

backend_kind() {
  case "$1" in
    ladybug|lancedb|cocoindex)
      echo "local"
      ;;
    surreal|falkor|pggraph)
      echo "docker"
      ;;
    sail|helix)
      echo "source"
      ;;
  esac
}

source_path_for() {
  case "$1" in
    sail) echo "${SAIL_SOURCE:-}" ;;
    surreal) echo "${SURREAL_SOURCE:-}" ;;
    falkor) echo "${FALKOR_SOURCE:-}" ;;
    helix) echo "${HELIX_SOURCE:-}" ;;
    pggraph) echo "${PGGRAPH_SOURCE:-}" ;;
    *) echo "" ;;
  esac
}

image_for() {
  case "$1" in
    surreal) echo "${SURREAL_IMAGE:-}" ;;
    falkor) echo "${FALKOR_IMAGE:-}" ;;
    pggraph) echo "${PGGRAPH_IMAGE:-}" ;;
    *) echo "" ;;
  esac
}

host_for() {
  case "$1" in
    sail) echo "$SAIL_HOST:$SAIL_PORT" ;;
    surreal) echo "$SURREAL_HOST:$SURREAL_PORT" ;;
    falkor) echo "$FALKOR_HOST:$FALKOR_PORT" ;;
    helix) echo "$HELIX_HOST:$HELIX_PORT" ;;
    pggraph) echo "$PGGRAPH_HOST:$PGGRAPH_PORT" ;;
    *) echo "local" ;;
  esac
}

doctor() {
  echo "Grust integration doctor"
  echo "  config:  $CONFIG"
  echo "  mode:    $MODE"
  echo "  profile: ${PROFILE:-custom/config}"
  echo "  backends: ${BACKENDS[*]}"
  echo
  if command -v cargo >/dev/null 2>&1; then
    echo "  cargo:   $(command -v cargo)"
  else
    echo "  cargo:   missing"
  fi
  if command -v docker >/dev/null 2>&1; then
    if docker info >/dev/null 2>&1; then
      echo "  docker:  available and running"
    else
      echo "  docker:  installed but not running"
    fi
  else
    echo "  docker:  missing"
  fi
  if command -v pg_isready >/dev/null 2>&1; then
    echo "  pg_isready: $(command -v pg_isready)"
  else
    echo "  pg_isready: missing; pgGraph readiness checks need PostgreSQL client tools"
  fi
  echo
  for backend in "${BACKENDS[@]}"; do
    local kind source host image state
    kind="$(backend_kind "$backend")"
    source="$(source_path_for "$backend")"
    host="$(host_for "$backend")"
    image="$(image_for "$backend")"
    state="will run local cargo integration test"
    case "$kind" in
      local)
        state="no daemon required"
        ;;
      docker)
        state="Docker Compose service available"
        if [[ -n "$image" ]]; then
          state="$state using $image"
        fi
        if [[ "$MODE" == "source" ]]; then
          state="source mode selected; start service manually or use auto/docker"
        fi
        ;;
      source)
        if [[ -n "$source" && -d "$source" ]]; then
          state="source checkout found at $source"
        elif [[ "$MODE" == "docker" ]]; then
          state="no configured Docker Compose service"
        else
          case "$backend" in
            sail) state="source checkout missing; start service manually or configure SAIL_SOURCE" ;;
            helix) state="source checkout missing; start service manually or configure HELIX_SOURCE" ;;
            *) state="source checkout missing; start service manually or configure the backend source path" ;;
          esac
        fi
        ;;
    esac
    if [[ "$host" != "local" ]]; then
      local host_name="${host%:*}"
      local port="${host##*:}"
      if port_open "$host_name" "$port"; then
        state="$state; already listening on $host"
        if [[ "$backend" == "pggraph" ]]; then
          if pggraph_has_graph_extension; then
            state="$state; graph extension available"
          else
            state="$state; graph extension unavailable, Docker mode will use a free fallback port"
          fi
        fi
      else
        state="$state; not listening on $host"
      fi
    fi
    printf '  %-9s %-7s %s\n' "$backend" "[$kind]" "$state"
  done
}

run_backend() {
  local backend="$1"
  case "$backend" in
    sail)
      start_sail
      wait_port sail "$SAIL_HOST" "$SAIL_PORT"
      cargo test -p grust-sail -- --ignored --test-threads=1
      ;;
    surreal)
      start_surreal
      wait_port surreal "$SURREAL_HOST" "$SURREAL_PORT"
      cargo test -p grust-surreal -- --ignored --test-threads=1
      ;;
    falkor)
      start_falkor
      wait_port falkor "$FALKOR_HOST" "$FALKOR_PORT"
      cargo test -p grust-falkor -- --ignored --test-threads=1
      ;;
    helix)
      start_helix
      wait_port helix "$HELIX_HOST" "$HELIX_PORT"
      cargo test -p grust-helix -- --ignored --test-threads=1
      ;;
    ladybug)
      cargo test -p grust-ladybug -- --test-threads=1
      ;;
    lancedb)
      cargo test -p grust-lancedb -- --ignored --test-threads=1
      ;;
    cocoindex)
      cargo test -p grust-cocoindex --test public_export
      ;;
    pggraph)
      start_pggraph
      wait_port pggraph "$PGGRAPH_HOST" "$PGGRAPH_PORT"
      wait_pggraph
      export PGGRAPH_TEST_CONNECTION_STRING="$PGGRAPH_CONNECTION_STRING"
      cargo test -p grust-pggraph -- --ignored --test-threads=1
      ;;
    *)
      echo "unknown backend: $backend" >&2
      exit 2
      ;;
  esac
}

if [[ "$DOCTOR" -eq 1 ]]; then
  doctor
  exit 0
fi

for backend in "${BACKENDS[@]}"; do
  run_backend "$backend"
done
