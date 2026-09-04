#!/usr/bin/env bash

# Bash 3.2-compatible creation and identity guards for benchmark artifacts.
# Callers must provide python3, which run-grust.sh already requires.

lsqb_normalize_directory_operand() {
    LSQB_NORMALIZED_DIRECTORY_OPERAND=$1
    while [[ "$LSQB_NORMALIZED_DIRECTORY_OPERAND" != "/" ]]; do
        case "$LSQB_NORMALIZED_DIRECTORY_OPERAND" in
            */)
                LSQB_NORMALIZED_DIRECTORY_OPERAND=${LSQB_NORMALIZED_DIRECTORY_OPERAND%/}
                ;;
            */.)
                LSQB_NORMALIZED_DIRECTORY_OPERAND=${LSQB_NORMALIZED_DIRECTORY_OPERAND%/.}
                [[ -n "$LSQB_NORMALIZED_DIRECTORY_OPERAND" ]] || \
                    LSQB_NORMALIZED_DIRECTORY_OPERAND=/
                ;;
            *) break ;;
        esac
    done
}

lsqb_require_regular_directory() {
    local path=$1 label=$2
    lsqb_normalize_directory_operand "$path"
    path=$LSQB_NORMALIZED_DIRECTORY_OPERAND
    if [[ ! -d "$path" || -L "$path" ]]; then
        echo "output-safety.sh: $label is not a regular non-symlink directory: $path" >&2
        return 1
    fi
}

lsqb_require_empty_directory() {
    local path=$1 label=$2
    lsqb_normalize_directory_operand "$path"
    path=$LSQB_NORMALIZED_DIRECTORY_OPERAND
    lsqb_require_regular_directory "$path" "$label" || return 1
    python3 - "$path" "$label" <<'PY'
import os
import stat
import sys

path = sys.argv[1]
label = sys.argv[2]
descriptor = None
try:
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0)
    flags |= getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    opened = os.fstat(descriptor)
    current = os.lstat(path)
    if (
        not stat.S_ISDIR(opened.st_mode)
        or stat.S_ISLNK(current.st_mode)
        or not stat.S_ISDIR(current.st_mode)
        or (opened.st_dev, opened.st_ino) != (current.st_dev, current.st_ino)
    ):
        raise SystemExit(
            f"output-safety.sh: {label} changed while it was inspected: {path}"
        )
    with os.scandir(descriptor) as entries:
        if next(entries, None) is not None:
            raise SystemExit(
                f"output-safety.sh: {label} is not empty; refusing to overwrite: {path}"
            )
except OSError as error:
    raise SystemExit(f"output-safety.sh: cannot inspect {label}: {path}: {error}")
finally:
    if descriptor is not None:
        os.close(descriptor)
PY
}

lsqb_ensure_regular_directory() {
    local path=$1 label=$2 create_parents=${3:-0}
    lsqb_normalize_directory_operand "$path"
    path=$LSQB_NORMALIZED_DIRECTORY_OPERAND
    if [[ -e "$path" || -L "$path" ]]; then
        lsqb_require_regular_directory "$path" "$label"
        return
    fi
    if [[ "$create_parents" == 1 ]]; then
        mkdir -p -- "$path" || return 1
    else
        mkdir -- "$path" || return 1
    fi
    lsqb_require_regular_directory "$path" "$label"
}

lsqb_reject_existing_output() {
    local path=$1 label=$2
    if [[ -e "$path" || -L "$path" ]]; then
        echo "output-safety.sh: refusing to overwrite $label: $path" >&2
        return 1
    fi
}

lsqb_directory_identity() {
    local path=$1
    lsqb_normalize_directory_operand "$path"
    path=$LSQB_NORMALIZED_DIRECTORY_OPERAND
    python3 - "$path" <<'PY'
import os
import stat
import sys

path = sys.argv[1]
metadata = os.lstat(path)
if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
    raise SystemExit(f"not a regular non-symlink directory: {path}")
print(f"{metadata.st_dev}:{metadata.st_ino}")
PY
}

lsqb_verify_directory_identity() {
    local path=$1 expected=$2 label=$3 actual
    actual=$(lsqb_directory_identity "$path") || {
        echo "output-safety.sh: cannot verify $label identity: $path" >&2
        return 1
    }
    if [[ "$actual" != "$expected" ]]; then
        echo "output-safety.sh: $label was replaced after creation: $path" >&2
        return 1
    fi
}

lsqb_verify_output_fd() {
    local descriptor=$1 path=$2 label=$3
    python3 - "$descriptor" "$path" "$label" <<'PY'
import os
import stat
import sys

descriptor = int(sys.argv[1])
path = sys.argv[2]
label = sys.argv[3]
try:
    opened = os.fstat(descriptor)
    current = os.lstat(path)
except OSError as error:
    raise SystemExit(f"output-safety.sh: cannot verify {label}: {path}: {error}")
if not stat.S_ISREG(opened.st_mode):
    raise SystemExit(f"output-safety.sh: opened {label} is not a regular file: {path}")
if stat.S_ISLNK(current.st_mode) or not stat.S_ISREG(current.st_mode):
    raise SystemExit(f"output-safety.sh: {label} became a symlink or special file: {path}")
if (opened.st_dev, opened.st_ino) != (current.st_dev, current.st_ino):
    raise SystemExit(f"output-safety.sh: {label} was replaced after creation: {path}")
PY
}

lsqb_close_output_fd() {
    local descriptor=$1 path=$2 label=$3 valid=0
    if lsqb_verify_output_fd "$descriptor" "$path" "$label"; then
        valid=1
    fi
    case "$descriptor" in
        3) exec 3>&- ;;
        4) exec 4>&- ;;
        5) exec 5>&- ;;
        6) exec 6>&- ;;
        *)
            echo "output-safety.sh: unsupported output descriptor: $descriptor" >&2
            return 2
            ;;
    esac
    (( valid == 1 ))
}

lsqb_open_exclusive_output_fd() {
    local descriptor=$1 path=$2 label=$3 parent name parent_identity temporary
    local linked=0 opened=0 valid=0
    lsqb_reject_existing_output "$path" "$label" || return 1
    parent=$(dirname -- "$path") || return 1
    name=$(basename -- "$path") || return 1
    lsqb_require_regular_directory "$parent" "$label parent" || return 1
    parent=$(cd -- "$parent" && pwd -P) || return 1
    parent_identity=$(lsqb_directory_identity "$parent") || return 1
    path="${parent}/${name}"
    lsqb_reject_existing_output "$path" "$label" || return 1
    temporary=$(mktemp "${parent}/.${name}.exclusive.XXXXXX") || return 1
    chmod 0644 "$temporary" || {
        rm -f -- "$temporary"
        return 1
    }
    if ln -- "$temporary" "$path"; then
        linked=1
    else
        rm -f -- "$temporary"
        echo "output-safety.sh: cannot exclusively install $label: $path" >&2
        return 1
    fi
    case "$descriptor" in
        3) if exec 3<>"$path"; then opened=1; fi ;;
        4) if exec 4<>"$path"; then opened=1; fi ;;
        5) if exec 5<>"$path"; then opened=1; fi ;;
        6) if exec 6<>"$path"; then opened=1; fi ;;
        *)
            echo "output-safety.sh: unsupported output descriptor: $descriptor" >&2
            ;;
    esac
    if (( opened == 1 )) && \
        lsqb_verify_directory_identity "$parent" "$parent_identity" "$label parent" && \
        lsqb_verify_output_fd "$descriptor" "$path" "$label" && \
        lsqb_verify_output_fd "$descriptor" "$temporary" "$label temporary file"; then
        valid=1
    fi
    if (( valid == 0 )); then
        if (( opened == 1 )); then
            case "$descriptor" in
                3) exec 3>&- ;;
                4) exec 4>&- ;;
                5) exec 5>&- ;;
                6) exec 6>&- ;;
            esac
        fi
        if (( linked == 1 )) && [[ -f "$path" && ! -L "$path" && \
            -f "$temporary" && ! -L "$temporary" && "$path" -ef "$temporary" ]]; then
            rm -f -- "$path"
        fi
        [[ ! -f "$temporary" || -L "$temporary" ]] || rm -f -- "$temporary"
        echo "output-safety.sh: cannot pin exclusively created $label: $path" >&2
        return 1
    fi
    if ! rm -f -- "$temporary"; then
        case "$descriptor" in
            3) exec 3>&- ;;
            4) exec 4>&- ;;
            5) exec 5>&- ;;
            6) exec 6>&- ;;
        esac
        if [[ -f "$path" && ! -L "$path" && "$path" -ef "$temporary" ]]; then
            rm -f -- "$path"
        fi
        return 1
    fi
}
