#!/usr/bin/env bash
# Resume mode for run-grust.sh: reuse a prior publication run's cells.
#
# A publication run normally executes every cell. With RESUME_FROM set to a
# prior OUTPUT_DIR, a cell whose component report, run log, watchdog record
# and (where the contract has one) service log exist there, were produced at
# the same source revision, runner image, service image and cell timeout,
# finished as a valid cell, and verify against the prior receipt's recorded
# hashes, is copied into the new output directory and not executed. Anything
# that fails any of those checks runs fresh. The prior directory itself is
# never modified.
#
# Sourced by run-grust.sh and by test-resume-cells.sh; every function reports
# a reason on stderr when it declines.

lsqb_resume_sha256() {
    local file=$1
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum -- "$file" | awk '{print $1}'
    else
        shasum -a 256 -- "$file" | awk '{print $1}'
    fi
}

lsqb_resume_regular_file() {
    local path=$1 label=$2
    if [[ -L "$path" || ! -f "$path" ]]; then
        echo "resume: ${label} is not a regular file: $path" >&2
        return 1
    fi
}

# lsqb_resume_load PRIOR_DIR REVISION SCALE
# Validates the prior directory's receipt and prints its SHA-256.
lsqb_resume_load() {
    local prior=$1 revision=$2 scale=$3 receipt receipt_revision receipt_scale
    if [[ -L "$prior" || ! -d "$prior" ]]; then
        echo "resume: RESUME_FROM is not a regular directory: $prior" >&2
        return 1
    fi
    receipt="${prior}/publication-receipt.json"
    lsqb_resume_regular_file "$receipt" "prior publication receipt" || return 1
    receipt_revision=$(jq -er '.source_revision' "$receipt" 2>/dev/null) || {
        echo "resume: prior receipt has no source revision: $receipt" >&2
        return 1
    }
    receipt_scale=$(jq -er '.scale_factor' "$receipt" 2>/dev/null) || {
        echo "resume: prior receipt has no scale factor: $receipt" >&2
        return 1
    }
    if [[ "$receipt_revision" != "$revision" ]]; then
        echo "resume: prior run is at revision ${receipt_revision}, not ${revision}" >&2
        return 1
    fi
    if [[ "$receipt_scale" != "$scale" ]]; then
        echo "resume: prior run is at scale ${receipt_scale}, not ${scale}" >&2
        return 1
    fi
    lsqb_resume_sha256 "$receipt"
}

# lsqb_resume_recorded_sha256 RECEIPT RELATIVE_PATH
# Prints the SHA-256 the prior receipt recorded for RELATIVE_PATH, or fails.
lsqb_resume_recorded_sha256() {
    local receipt=$1 relative=$2 recorded
    recorded=$(jq -er --arg path "$relative" \
        '[.output_inventory[] | select(.path == $path)] | if length == 1 then .[0].sha256 else empty end' \
        "$receipt" 2>/dev/null) || return 1
    [[ "$recorded" =~ ^[0-9a-f]{64}$ ]] || return 1
    printf '%s\n' "$recorded"
}

# lsqb_resume_cell PRIOR_DIR OUTPUT_DIR SUITE BACKEND SCALE REVISION
#                  CELL_TIMEOUT_MS FEATURE RUNNER_IMAGE RUNNER_IMAGE_ID
#                  SERVICE_IMAGE SERVICE_IMAGE_ID HAS_SERVICE_LOG
# Copies the cell's files from PRIOR_DIR into OUTPUT_DIR when every check
# passes and prints the prior watchdog record's Compose project. Returns 1,
# with nothing copied, when the cell must run fresh.
lsqb_resume_cell() {
    local prior=$1 output=$2 suite=$3 backend=$4 scale=$5 revision=$6
    local cell_timeout_ms=$7 feature=$8 runner_image=$9 runner_image_id=${10}
    local service_image=${11} service_image_id=${12} has_service_log=${13}
    local cell="${suite}-${backend}" receipt="${prior}/publication-receipt.json"
    local relative recorded actual project row expected_row
    local -a files=(
        "components/${cell}-sf${scale}.json"
        "logs/${cell}.log"
        "watchdogs/${cell}.json"
    )
    if [[ "$has_service_log" == 1 ]]; then
        files+=("logs/${cell}-service.log")
    elif lsqb_resume_recorded_sha256 "$receipt" "logs/${cell}-service.log" >/dev/null; then
        echo "resume: ${cell} had a service log in the prior run but has none under this contract" >&2
        return 1
    fi

    for relative in "${files[@]}"; do
        lsqb_resume_regular_file "${prior}/${relative}" "prior ${cell} output" || return 1
        recorded=$(lsqb_resume_recorded_sha256 "$receipt" "$relative") || {
            echo "resume: prior receipt records no hash for ${relative}" >&2
            return 1
        }
        actual=$(lsqb_resume_sha256 "${prior}/${relative}")
        if [[ "$actual" != "$recorded" ]]; then
            echo "resume: ${relative} differs from the prior receipt's recorded hash" >&2
            return 1
        fi
        if [[ -e "${output}/${relative}" || -L "${output}/${relative}" ]]; then
            echo "resume: ${relative} already exists in the output directory" >&2
            return 1
        fi
    done

    expected_row=$(printf '%s\t%s\t%s\t%s\t%s\t%s\t%s' "$suite" "$backend" \
        "$feature" "$runner_image" "$runner_image_id" "$service_image" "$service_image_id")
    row=$(awk -F'\t' -v suite="$suite" -v backend="$backend" \
        '$1 == suite && $2 == backend' "${prior}/images.tsv" 2>/dev/null) || row=
    if [[ "$row" != "$expected_row" ]]; then
        echo "resume: ${cell} ran on different images in the prior run" >&2
        return 1
    fi

    if ! jq -e --arg revision "$revision" --argjson timeout "$cell_timeout_ms" \
        '.environment.grust_revision == $revision
            and .timing.cell_timeout_ms == $timeout
            and .valid == true' \
        "${prior}/${files[0]}" >/dev/null 2>&1; then
        echo "resume: ${cell} component is not a valid cell at this revision and timeout" >&2
        return 1
    fi
    project=$(jq -er --argjson timeout "$cell_timeout_ms" \
        'select(.status == "complete" and .child_exit_status == 0 and .timeout_ms == $timeout)
            | .project' "${prior}/${files[2]}" 2>/dev/null) || {
        echo "resume: ${cell} watchdog record is not a clean completion at this timeout" >&2
        return 1
    }
    [[ "$project" =~ ^grust-lsqb-matrix-[0-9]+-[0-9]+$ ]] || {
        echo "resume: ${cell} watchdog record has an invalid Compose project" >&2
        return 1
    }

    for relative in "${files[@]}"; do
        cp -- "${prior}/${relative}" "${output}/${relative}" || {
            echo "resume: cannot copy ${relative}" >&2
            return 1
        }
        recorded=$(lsqb_resume_recorded_sha256 "$receipt" "$relative")
        actual=$(lsqb_resume_sha256 "${output}/${relative}")
        if [[ "$actual" != "$recorded" ]]; then
            echo "resume: copied ${relative} does not match the recorded hash" >&2
            return 1
        fi
    done
    printf '%s\n' "$project"
}
