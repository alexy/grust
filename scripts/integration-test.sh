#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

CONFIG="${GRUST_INTEGRATION_CONFIG:-integration/backends.conf}"
if [[ -f "$CONFIG" ]]; then
  # shellcheck disable=SC1090
  source "$CONFIG"
fi

BACKENDS=()
NO_START=0
KEEP_RUNNING=0

usage() {
  cat <<'USAGE'
Usage: scripts/integration-test.sh [--backend NAME ...] [--no-start] [--keep-running]

Starts configured local backend services when needed, then runs live backend
tests. A live test fails if its backend is absent; no successful run is produced
by silently skipping an unavailable service.

Backends: sail, surreal, falkor, helix, lancedb, cocoindex, pggraph

Config: integration/backends.conf
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --backend)
      BACKENDS+=("${2:?missing backend name}")
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

if [[ ${#BACKENDS[@]} -eq 0 ]]; then
  # shellcheck disable=SC2206
  BACKENDS=(${GRUST_INTEGRATION_BACKENDS:-sail surreal falkor helix lancedb cocoindex pggraph})
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

start_sail() {
  if port_open "$SAIL_HOST" "$SAIL_PORT"; then
    echo "sail already listening on $SAIL_HOST:$SAIL_PORT"
    return
  fi
  [[ "$NO_START" -eq 0 ]] || return
  if [[ -n "${SAIL_SOURCE:-}" && -d "$SAIL_SOURCE" ]]; then
    if [[ -x "$SAIL_SOURCE/target/release/sail" ]]; then
      start_process sail "$SAIL_SOURCE" "$SAIL_SOURCE/target/release/sail" spark server --port "$SAIL_PORT"
    elif command -v sail >/dev/null 2>&1; then
      start_process sail "$SAIL_SOURCE" sail spark server --port "$SAIL_PORT"
    elif command -v hatch >/dev/null 2>&1; then
      start_process sail "$SAIL_SOURCE" hatch run sail spark server --port "$SAIL_PORT"
    else
      echo "no Sail launcher found; build / install Sail or start it manually" >&2
    fi
  fi
}

start_surreal() {
  if port_open "$SURREAL_HOST" "$SURREAL_PORT"; then
    echo "surreal already listening on $SURREAL_HOST:$SURREAL_PORT"
    return
  fi
  [[ "$NO_START" -eq 0 ]] || return
  if [[ -n "${SURREAL_SOURCE:-}" && -d "$SURREAL_SOURCE" ]]; then
    start_process surreal "$SURREAL_SOURCE" cargo run --no-default-features --features storage-mem,http,scripting -- start --log info --user root --pass root memory
  elif command -v docker >/dev/null 2>&1; then
    start_compose surreal
  fi
}

start_falkor() {
  if port_open "$FALKOR_HOST" "$FALKOR_PORT"; then
    echo "falkor already listening on $FALKOR_HOST:$FALKOR_PORT"
    return
  fi
  [[ "$NO_START" -eq 0 ]] || return
  if [[ -n "${FALKOR_SOURCE:-}" && -d "$FALKOR_SOURCE" && -f "$FALKOR_SOURCE/Makefile" && -x "$(command -v redis-server || true)" ]]; then
    start_process falkor "$FALKOR_SOURCE" make run
  elif command -v docker >/dev/null 2>&1; then
    start_compose falkor
  fi
}

start_helix() {
  if port_open "$HELIX_HOST" "$HELIX_PORT"; then
    echo "helix already listening on $HELIX_HOST:$HELIX_PORT"
    return
  fi
  [[ "$NO_START" -eq 0 ]] || return
  if [[ -n "${HELIX_SOURCE:-}" && -d "$HELIX_SOURCE" ]]; then
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
  fi
}

start_pggraph() {
  if port_open "$PGGRAPH_HOST" "$PGGRAPH_PORT"; then
    echo "pggraph/postgres already listening on $PGGRAPH_HOST:$PGGRAPH_PORT"
    return
  fi
  [[ "$NO_START" -eq 0 ]] || return
  if [[ -n "${PGGRAPH_IMAGE:-}" ]] && command -v docker >/dev/null 2>&1; then
    start_compose pggraph
  else
    echo "pggraph startup is not configured yet; start PostgreSQL with the graph extension manually or set PGGRAPH_IMAGE" >&2
  fi
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

for backend in "${BACKENDS[@]}"; do
  run_backend "$backend"
done
