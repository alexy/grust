#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
exec "$HOME/src/firstpair/publishing/scripts/build-library-book.sh" \
  --repo-root "$repo_root" \
  "$@"
