#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=benchmarks/lsqb/dataset-integrity.sh
source "${script_dir}/dataset-integrity.sh"
scale=0.1
data_root="${script_dir}/data"
archive_source=

usage() {
    cat <<'USAGE'
Usage: fetch-dataset.sh [--scale 0.1|0.3] [--data-root DIRECTORY]
                        [--archive FILE]

Download and verify an official LSQB projected-FK dataset. The destination is
DIRECTORY/social-network-sf<SCALE>-projected-fk and must not already exist.
When --archive is supplied, verify and extract that local archive instead of
downloading another copy.

Defaults:
  --scale 0.1
  --data-root benchmarks/lsqb/data
USAGE
}

die() {
    echo "fetch-dataset.sh: $*" >&2
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --scale)
            [[ $# -ge 2 ]] || die "--scale requires a value"
            scale=$2
            shift 2
            ;;
        --data-root)
            [[ $# -ge 2 ]] || die "--data-root requires a directory"
            data_root=$2
            shift 2
            ;;
        --archive)
            [[ $# -ge 2 ]] || die "--archive requires a file"
            archive_source=$2
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            die "unknown argument: $1"
            ;;
    esac
done

case "$scale" in
    0.1|0.3) ;;
    *) die "unsupported scale factor '$scale'; choose 0.1 or 0.3" ;;
esac
lsqb_set_expected_dataset "$scale" || die "no pinned provenance for scale factor '$scale'"
expected_sha256=$LSQB_EXPECTED_ARCHIVE_SHA256
expected_bytes=$LSQB_EXPECTED_ARCHIVE_BYTES

for command in tar mktemp; do
    command -v "$command" >/dev/null 2>&1 || die "required command not found: $command"
done
if [[ -z "$archive_source" ]]; then
    command -v curl >/dev/null 2>&1 || die "required command not found: curl"
elif [[ ! -f "$archive_source" ]]; then
    die "local archive does not exist or is not a regular file: $archive_source"
fi
if command -v sha256sum >/dev/null 2>&1; then
    sha256() {
        sha256sum "$1" | awk '{print $1}'
    }
elif command -v shasum >/dev/null 2>&1; then
    sha256() {
        shasum -a 256 "$1" | awk '{print $1}'
    }
else
    die "required SHA-256 tool not found (sha256sum or shasum)"
fi

dataset="social-network-sf${scale}-projected-fk"
archive_name="${dataset}.tar.zst"
source_url="https://datasets.ldbcouncil.org/lsqb/${archive_name}"

if [[ -e "$data_root" && ! -d "$data_root" ]]; then
    die "data root exists but is not a directory: $data_root"
fi
mkdir -p -- "$data_root"
data_root=$(cd -- "$data_root" && pwd -P)
destination="${data_root}/${dataset}"
if [[ -e "$destination" || -L "$destination" ]]; then
    die "destination already exists; refusing to merge or overwrite: $destination"
fi

work_dir=$(mktemp -d "${data_root}/.lsqb-fetch.XXXXXX")
cleanup() {
    if [[ -n "${work_dir:-}" && -d "$work_dir" ]]; then
        case "$work_dir" in
            "${data_root}"/.lsqb-fetch.*) rm -rf -- "$work_dir" ;;
            *) echo "fetch-dataset.sh: refusing unsafe cleanup path: $work_dir" >&2 ;;
        esac
    fi
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

archive="${work_dir}/${archive_name}"
if [[ -n "$archive_source" ]]; then
    archive_source=$(cd -- "$(dirname -- "$archive_source")" && pwd -P)/$(basename -- "$archive_source")
    echo "Using local archive ${archive_source}"
    cp -- "$archive_source" "$archive"
else
    echo "Downloading ${source_url}"
    curl \
        --fail \
        --location \
        --proto '=https' \
        --retry 3 \
        --show-error \
        --silent \
        --tlsv1.2 \
        --output "$archive" \
        "$source_url"
fi

actual_bytes=$(wc -c < "$archive" | tr -d '[:space:]')
[[ "$actual_bytes" == "$expected_bytes" ]] || die \
    "archive size mismatch: expected $expected_bytes bytes, received $actual_bytes"
actual_sha256=$(sha256 "$archive")
[[ "$actual_sha256" == "$expected_sha256" ]] || die \
    "archive checksum mismatch: expected $expected_sha256, received $actual_sha256"

archive_list="${work_dir}/archive.list"
archive_verbose="${work_dir}/archive.verbose"
tar_input=$archive
if command -v zstd >/dev/null 2>&1; then
    tar_input="${work_dir}/${dataset}.tar"
    zstd --decompress --stdout -- "$archive" > "$tar_input" || \
        die "cannot decompress verified archive with zstd"
elif command -v unzstd >/dev/null 2>&1; then
    tar_input="${work_dir}/${dataset}.tar"
    unzstd --stdout -- "$archive" > "$tar_input" || \
        die "cannot decompress verified archive with unzstd"
elif ! tar -tf "$archive" >/dev/null 2>&1; then
    die "tar cannot read .tar.zst archives and neither unzstd nor zstd is installed"
fi

tar -tf "$tar_input" > "$archive_list" || die "cannot list verified archive"
tar -tvf "$tar_input" > "$archive_verbose" || \
    die "cannot inspect verified archive types"

entry_count=0
while IFS= read -r entry || [[ -n "$entry" ]]; do
    [[ -n "$entry" ]] || die "archive contains an empty path"
    entry_count=$((entry_count + 1))
    case "$entry" in
        /*|..|../*|*/../*|*/..|*/./*|*/.|*//*|*\\*)
            die "archive contains an unsafe path: $entry"
            ;;
    esac
    case "$entry" in
        "$dataset"|"$dataset/"|"$dataset/"*) ;;
        *) die "archive entry is outside the expected dataset root: $entry" ;;
    esac
done < "$archive_list"
[[ "$entry_count" -gt 0 ]] || die "archive contains no entries"

while IFS= read -r entry || [[ -n "$entry" ]]; do
    case "${entry:0:1}" in
        -|d) ;;
        *) die "archive contains a non-file, non-directory entry: $entry" ;;
    esac
done < "$archive_verbose"

extract_root="${work_dir}/extract"
mkdir -- "$extract_root"
tar -xf "$tar_input" -C "$extract_root"
staged_dataset="${extract_root}/${dataset}"
[[ -d "$staged_dataset" ]] || die "archive did not create expected root: $dataset"
[[ -f "$staged_dataset/Person.csv" ]] || die "archive is missing Person.csv"
[[ -f "$staged_dataset/Person_knows_Person.csv" ]] || \
    die "archive is missing Person_knows_Person.csv"
lsqb_verify_dataset "$scale" "$staged_dataset" || \
    die "extracted dataset does not match the pinned official manifest"
printf '%s\n' \
    'schema=grust-lsqb-dataset-v1' \
    "scale=${scale}" \
    "archive_sha256=${expected_sha256}" \
    "archive_bytes=${expected_bytes}" \
    "extracted_manifest_sha256=${LSQB_EXPECTED_MANIFEST_SHA256}" \
    "csv_files=${LSQB_EXPECTED_CSV_FILES}" \
    "csv_bytes=${LSQB_EXPECTED_CSV_BYTES}" \
    > "${staged_dataset}/.grust-lsqb-verified"

# Check again immediately before the single final rename. A pre-existing path is
# never merged, replaced, or treated as a cache hit.
if [[ -e "$destination" || -L "$destination" ]]; then
    die "destination appeared during download; refusing to overwrite: $destination"
fi
mv -- "$staged_dataset" "$destination"

echo "Verified SHA-256: $actual_sha256"
echo "Verified extracted manifest: $LSQB_EXPECTED_MANIFEST_SHA256"
echo "Installed dataset: $destination"
