# Grust Integration Tests

Grust has two kinds of tests:

- Unit and offline tests that run with `cargo test --workspace --all-features`.
- Backend integration tests that talk to real backend services or real local
  backend storage.

Integration tests must never silently pass because a backend is absent. The
launcher in `scripts/integration-test.sh` either starts the backend, uses an
already-running backend, or fails.

## New-User Path

For a first integration run, use Docker mode:

```sh
scripts/integration-test.sh doctor --profile docker --mode docker
scripts/integration-test.sh --profile docker --mode docker
```

The Docker profile runs every backend that is currently reproducible without a
source checkout:

- SurrealDB through Docker Compose.
- FalkorDB through Docker Compose.
- pgGraph through Docker Compose.
- LadybugDB as an embedded local integration test.
- LanceDB as a local storage integration test.
- CocoIndex as a local export integration test.

Sail and HelixDB are not part of the default Docker profile today because the
repository does not have a pinned, verified Docker startup contract for them.
They remain covered by the full maintainer profile through source checkouts or
already-running services.

## Profiles

Profiles choose which backends to test:

```sh
scripts/integration-test.sh --profile quick
scripts/integration-test.sh --profile docker --mode docker
scripts/integration-test.sh --profile all
```

`quick` runs only local integration checks that do not need daemons:

- LadybugDB
- LanceDB
- CocoIndex

`docker` is the contributor-friendly profile:

- SurrealDB
- FalkorDB
- LadybugDB
- LanceDB
- CocoIndex
- pgGraph

`all` is the maintainer profile:

- Sail
- SurrealDB
- FalkorDB
- HelixDB
- LadybugDB
- LanceDB
- CocoIndex
- pgGraph

You can also target one backend directly:

```sh
scripts/integration-test.sh --backend pggraph --mode docker
scripts/integration-test.sh --backend helix --mode source
```

## Modes

Modes choose how services are started:

```sh
scripts/integration-test.sh --mode auto
scripts/integration-test.sh --mode docker
scripts/integration-test.sh --mode source
```

`auto` is the maintainer convenience mode. It uses an already-running service
if the port is open, otherwise it prefers configured source checkouts, then
falls back to Docker Compose where a Docker service exists.

`docker` is the new-user mode. It does not use local source checkouts. It starts
Docker Compose services for Docker-backed backends and runs local checks for
LanceDB and CocoIndex.

`source` is for testing against local backend checkouts. It does not start
Docker services.

Use `--no-start` to require services to already be running, and
`--keep-running` to leave launcher-started services up for debugging.

## Doctor

Run doctor before a long integration pass:

```sh
scripts/integration-test.sh doctor --profile docker --mode docker
scripts/integration-test.sh doctor --profile all
```

Doctor reports:

- the selected profile and mode;
- whether Docker is installed and running;
- whether `cargo` and `pg_isready` are available;
- which backends are selected;
- whether a backend port is already listening;
- whether a source checkout is configured for source-backed backends.

## Docker Compose

`docker-compose.integration.yml` contains the Docker-backed services:

- `surreal`
- `falkor`
- `pggraph`

The defaults are pinned for reproducibility:

```sh
SURREAL_IMAGE=surrealdb/surrealdb:v3.1
FALKOR_IMAGE=falkordb/falkordb:v4.18.10
PGGRAPH_IMAGE=ghcr.io/evokoa/pggraph:0.1.7
```

To intentionally test against latest backend images:

```sh
GRUST_INTEGRATION_IMAGE_CHANNEL=latest \
  scripts/integration-test.sh --profile docker --mode docker
```

You can also override individual images:

```sh
SURREAL_IMAGE=surrealdb/surrealdb:latest \
  scripts/integration-test.sh --backend surreal --mode docker
```

## Source Checkouts

The full maintainer profile can use local backend checkouts configured in
`integration/backends.conf`:

```sh
SAIL_SOURCE=/Users/alexy/src/sail
SURREAL_SOURCE=/Users/alexy/src/SurrealDB
FALKOR_SOURCE=/Users/alexy/src/FalkorDB
HELIX_SOURCE=/Users/alexy/src/HelixDB
```

These defaults match the maintainer workstation, but contributors can override
them in the environment:

```sh
SAIL_SOURCE=$HOME/src/sail scripts/integration-test.sh --backend sail
```

Set a source variable to an empty value to prevent auto mode from using that
checkout:

```sh
SAIL_SOURCE= scripts/integration-test.sh --profile all
```

## Backend Notes

Sail currently runs through a local Sail checkout, an installed `sail` binary,
or `hatch run sail spark server`. Add a Docker Compose service only after a
pinned Sail image and command have been verified.

HelixDB currently runs through a local Helix checkout or installed `helix`
binary. The launcher creates a disposable Helix project under the integration
state directory unless `HELIX_PROJECT_DIR` is set.

SurrealDB and FalkorDB can run either from source checkouts or Docker Compose.

pgGraph runs through Docker Compose by default because it needs PostgreSQL with
the pgGraph extension installed. The default connection string points at host
port `55432` so it does not collide with a developer's ordinary PostgreSQL on
`5432`.

If a PostgreSQL-compatible port is already listening but the `graph` extension
is not available there, the launcher does not treat that as a usable pgGraph
backend. In Docker-capable modes it automatically picks a free fallback port
starting at `55432`, rewrites the pgGraph connection string for that run, and
starts the Compose pgGraph service there:

```sh
PGGRAPH_PORT=5432 scripts/integration-test.sh --backend pggraph --mode docker
```

That command will use the local service on `5432` only if it exposes the
`graph` extension. Otherwise it will start Grust's own pgGraph container on a
free high port such as `55432`.

LadybugDB, LanceDB, and CocoIndex do not need daemon startup. LadybugDB runs
through the embedded Rust `lbug` crate, LanceDB exercises real local storage,
and CocoIndex exercises export/import behavior. All three are included in
Docker and all profiles.

## CI Strategy

A practical CI setup should have at least two lanes:

```sh
cargo test --workspace --all-features
scripts/integration-test.sh --profile docker --mode docker
```

For compatibility monitoring, add a non-blocking or scheduled latest-image
lane:

```sh
GRUST_INTEGRATION_IMAGE_CHANNEL=latest \
  scripts/integration-test.sh --profile docker --mode docker
```

Keep the normal contributor lane pinned. Let the latest-image lane tell us when
upstream changed behavior.
