"""Load explicitly pinned historical contracts, never the mutable current one.

The retained SDK manifest is git blob 66e374bd07505e11ca4729d844addd2c71d5dbc6
at both qualified SDK source revisions. Runtime selection uses its SHA-256;
it does not depend on Git, an output directory, or an external manifest path.
"""

import hashlib
import json
import os
from pathlib import Path
import stat


CONTRACT_DIRECTORY = Path(__file__).resolve().with_name("contracts")
KNOWN_MANIFESTS = frozenset({
    "1dcae942840f216a83282f45f27e7fe228616e8f51af764689dc4f4fea0de849",
})


def _read_regular(path: Path) -> bytes:
    try:
        metadata = path.lstat()
        if not stat.S_ISREG(metadata.st_mode):
            raise ValueError("historical contract is not a regular non-symlink file")
        flags = (os.O_RDONLY | getattr(os, "O_CLOEXEC", 0)
                 | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0))
        with os.fdopen(os.open(path, flags), "rb") as stream:
            opened = os.fstat(stream.fileno())
            if (not stat.S_ISREG(opened.st_mode)
                    or (opened.st_dev, opened.st_ino) != (metadata.st_dev, metadata.st_ino)):
                raise ValueError("historical contract changed while it was opened")
            return stream.read()
    except OSError as error:
        raise ValueError(f"historical contract is unavailable: {path}") from error


def _unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("historical contract JSON contains duplicate keys")
        result[key] = value
    return result


def _reject_constant(_value):
    raise ValueError("historical contract JSON contains a nonfinite number")


def load_manifest(digest: str) -> dict:
    """Authenticate one allowlisted contract's captured bytes before parsing."""
    if not isinstance(digest, str) or digest not in KNOWN_MANIFESTS:
        raise ValueError("unknown historical contract digest")
    if CONTRACT_DIRECTORY.is_symlink():
        raise ValueError("historical contract directory must not be a symlink")
    raw = _read_regular(CONTRACT_DIRECTORY / f"{digest}.json")
    if hashlib.sha256(raw).hexdigest() != digest:
        raise ValueError("historical contract digest mismatch")
    try:
        manifest = json.loads(raw, object_pairs_hook=_unique_object,
                              parse_constant=_reject_constant)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError("historical contract contains invalid JSON") from error
    if (not isinstance(manifest, dict)
            or manifest.get("schema") != "grust-lsqb-evidence-manifest-v2"):
        raise ValueError("historical contract has an unexpected manifest schema")
    return manifest
