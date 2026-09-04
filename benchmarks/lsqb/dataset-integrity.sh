#!/usr/bin/env bash
# shellcheck disable=SC2034 # Expected constants are consumed by sourcing scripts.

# Shared provenance constants and the byte-for-byte manifest algorithm used by
# the Rust matrix runner. This file is sourced by the fetch and run scripts;
# keep its constants synchronized with src/provenance.rs, which independently
# enforces them at the report boundary.

lsqb_set_expected_dataset() {
    case "$1" in
        0.1)
            LSQB_EXPECTED_ARCHIVE_SHA256=20b08cfbc0b765bb066135a4c8d99367fb4f0d5c500a63b725e258dcb91b7005
            LSQB_EXPECTED_ARCHIVE_BYTES=6362514
            LSQB_EXPECTED_MANIFEST_SHA256=c0d76ea897df030f901c7436d2d7ee0cd31591db54c3c6c311d79a68fa138085
            LSQB_EXPECTED_CSV_FILES=36
            LSQB_EXPECTED_CSV_BYTES=53863509
            ;;
        0.3)
            LSQB_EXPECTED_ARCHIVE_SHA256=4aad6e31047a356d40e8c315916c3fe35a77911024136d69868b39b16f8ccf33
            LSQB_EXPECTED_ARCHIVE_BYTES=19134337
            LSQB_EXPECTED_MANIFEST_SHA256=aeb94da1177ca732b127574116d7624b131113ffc7f6f8e612b0bb2dab31d5f3
            LSQB_EXPECTED_CSV_FILES=36
            LSQB_EXPECTED_CSV_BYTES=160662563
            ;;
        *) return 1 ;;
    esac
}

lsqb_sha256_stdin() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 | awk '{print $1}'
    else
        echo "no SHA-256 implementation found (sha256sum or shasum)" >&2
        return 1
    fi
}

lsqb_emit_u64_be() {
    local value=$1 shift byte octal
    for shift in 56 48 40 32 24 16 8 0; do
        byte=$(( (value >> shift) & 255 ))
        printf -v octal '%03o' "$byte"
        printf '%b' "\\${octal}"
    done
}

# Print: SHA-256<TAB>CSV-file-count<TAB>CSV-byte-count.
lsqb_dataset_manifest() {
    local directory=$1 path name byte_len digest
    local csv_files=0 csv_bytes=0
    local LC_ALL=C
    local -a paths

    for path in awk cat tr wc; do
        command -v "$path" >/dev/null 2>&1 || {
            echo "required manifest command not found: $path" >&2
            return 1
        }
    done
    [[ -d "$directory" ]] || {
        echo "dataset directory does not exist: $directory" >&2
        return 1
    }
    paths=("$directory"/*.csv)
    [[ -f "${paths[0]}" && ! -L "${paths[0]}" ]] || {
        echo "dataset contains no regular CSV files: $directory" >&2
        return 1
    }
    for path in "${paths[@]}"; do
        [[ -f "$path" && ! -L "$path" ]] || {
            echo "dataset CSV is not a regular non-symlink file: $path" >&2
            return 1
        }
        byte_len=$(wc -c < "$path" | tr -d '[:space:]')
        [[ "$byte_len" =~ ^[0-9]+$ ]] || {
            echo "cannot determine dataset CSV size: $path" >&2
            return 1
        }
        csv_files=$((csv_files + 1))
        csv_bytes=$((csv_bytes + byte_len))
    done

    digest=$(
        {
            printf 'grust-lsqb-projected-fk-manifest-v1\0'
            for path in "${paths[@]}"; do
                name=${path##*/}
                byte_len=$(wc -c < "$path" | tr -d '[:space:]')
                lsqb_emit_u64_be "${#name}"
                printf '%s' "$name"
                lsqb_emit_u64_be "$byte_len"
                command cat -- "$path"
            done
        } | lsqb_sha256_stdin
    ) || return 1

    printf '%s\t%s\t%s\n' "$digest" "$csv_files" "$csv_bytes"
}

lsqb_verify_dataset() {
    local scale=$1 directory=$2 manifest actual_sha256 actual_files actual_bytes
    lsqb_set_expected_dataset "$scale" || {
        echo "no pinned dataset manifest for scale factor: $scale" >&2
        return 1
    }
    manifest=$(lsqb_dataset_manifest "$directory") || return 1
    IFS=$'\t' read -r actual_sha256 actual_files actual_bytes <<< "$manifest"
    [[ "$actual_sha256" == "$LSQB_EXPECTED_MANIFEST_SHA256" ]] || {
        echo "extracted manifest mismatch: expected $LSQB_EXPECTED_MANIFEST_SHA256, received $actual_sha256" >&2
        return 1
    }
    [[ "$actual_files" == "$LSQB_EXPECTED_CSV_FILES" ]] || {
        echo "CSV file count mismatch: expected $LSQB_EXPECTED_CSV_FILES, received $actual_files" >&2
        return 1
    }
    [[ "$actual_bytes" == "$LSQB_EXPECTED_CSV_BYTES" ]] || {
        echo "CSV byte count mismatch: expected $LSQB_EXPECTED_CSV_BYTES, received $actual_bytes" >&2
        return 1
    }
}

# Authenticate the fetch receipt after lsqb_verify_dataset has independently
# recomputed the extracted CSV manifest. On success, expose its exact digest in
# LSQB_VERIFIED_RECEIPT_SHA256 for result provenance.
lsqb_verify_dataset_receipt() {
    local scale=$1 directory=$2 receipt expected_digest actual_digest
    lsqb_set_expected_dataset "$scale" || {
        echo "no pinned dataset receipt for scale factor: $scale" >&2
        return 1
    }
    receipt="${directory}/.grust-lsqb-verified"
    [[ -f "$receipt" && ! -L "$receipt" ]] || {
        echo "dataset receipt is not a regular non-symlink file: $receipt" >&2
        return 1
    }
    expected_digest=$(
        printf '%s\n' \
            'schema=grust-lsqb-dataset-v1' \
            "scale=${scale}" \
            "archive_sha256=${LSQB_EXPECTED_ARCHIVE_SHA256}" \
            "archive_bytes=${LSQB_EXPECTED_ARCHIVE_BYTES}" \
            "extracted_manifest_sha256=${LSQB_EXPECTED_MANIFEST_SHA256}" \
            "csv_files=${LSQB_EXPECTED_CSV_FILES}" \
            "csv_bytes=${LSQB_EXPECTED_CSV_BYTES}" | \
            lsqb_sha256_stdin
    ) || return 1
    actual_digest=$(lsqb_sha256_stdin < "$receipt") || return 1
    [[ "$actual_digest" == "$expected_digest" ]] || {
        echo "dataset receipt does not match pinned provenance for scale $scale" >&2
        return 1
    }
    LSQB_VERIFIED_RECEIPT_SHA256=$actual_digest
}

# Copy only the authenticated CSV payload and receipt into a private snapshot
# root, make the copy read-only, then independently re-verify it. Callers mount
# this snapshot rather than the mutable user-facing download directory.
# On success LSQB_DATASET_SNAPSHOT_DIRECTORY names the copied dataset directory.
lsqb_create_dataset_snapshot() {
    local scale=$1 source_directory=$2 snapshot_root=$3 dataset destination path
    local -a csv_paths
    for path in chmod cp find mkdir; do
        command -v "$path" >/dev/null 2>&1 || {
            echo "required snapshot command not found: $path" >&2
            return 1
        }
    done
    [[ -d "$snapshot_root" && ! -L "$snapshot_root" ]] || {
        echo "dataset snapshot root is not a regular directory: $snapshot_root" >&2
        return 1
    }
    [[ -z $(find "$snapshot_root" -mindepth 1 -maxdepth 1 -print -quit) ]] || {
        echo "dataset snapshot root is not empty: $snapshot_root" >&2
        return 1
    }
    lsqb_verify_dataset "$scale" "$source_directory" || return 1
    lsqb_verify_dataset_receipt "$scale" "$source_directory" || return 1

    dataset="social-network-sf${scale}-projected-fk"
    destination="${snapshot_root}/${dataset}"
    mkdir -- "$destination" || return 1
    csv_paths=("$source_directory"/*.csv)
    for path in "${csv_paths[@]}"; do
        cp -p -- "$path" "$destination/" || return 1
    done
    cp -p -- "$source_directory/.grust-lsqb-verified" "$destination/" || return 1
    chmod 0444 "$destination"/*.csv "$destination/.grust-lsqb-verified" || return 1
    chmod 0555 "$destination" || return 1

    lsqb_verify_dataset "$scale" "$destination" || return 1
    lsqb_verify_dataset_receipt "$scale" "$destination" || return 1
    LSQB_DATASET_SNAPSHOT_DIRECTORY=$destination
}
