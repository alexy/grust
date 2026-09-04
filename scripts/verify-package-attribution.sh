#!/usr/bin/env bash
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo"

package_id=$(cargo pkgid -p grust-graph)
version=${package_id##*@}
archive="target/package/grust-graph-${version}.crate"

if [[ ! -f "$archive" ]]; then
  echo "missing packaged facade archive: $archive" >&2
  exit 1
fi

prefix="grust-graph-${version}"
required=(
  "$prefix/THIRD_PARTY/apache-ossie/LICENSE-2.0.txt"
  "$prefix/THIRD_PARTY/apache-ossie/NOTICE"
  "$prefix/tests/fixtures/apache-ossie-tpcds-ddb19f1b.yaml"
)
entries=$(tar -tzf "$archive")
for path in "${required[@]}"; do
  if ! grep -Fqx "$path" <<<"$entries"; then
    echo "packaged facade is missing required attribution file: $path" >&2
    exit 1
  fi
done

echo "verified Apache Ossie fixture attribution in $archive"
