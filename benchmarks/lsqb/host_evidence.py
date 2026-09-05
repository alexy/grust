"""Versioned, receipt-bound startup screening; never ongoing host qualification."""

from datetime import datetime, timedelta
import json
import math
import re


SCHEMA = "grust-host-preflight-v1"
FILENAME = "host-preflight.json"
LIMITATION = "startup screen only; ongoing contention monitoring required"
RECORD_FIELDS = {
    "schema", "samples", "clean_host_performance_eligible", "limitation",
    "startup_screen_passed",
}
# Records written before the explicit limit existed omit this field and are
# held to the original two-core screen.
LIMIT_FIELD = "total_cpu_limit_percent"
DEFAULT_TOTAL_CPU_LIMIT = 200
MAX_TOTAL_CPU_LIMIT = 400
SAMPLE_FIELDS = {
    "total_cpu_percent", "busy_processes", "startup_screen_passed", "observed_at",
}
UTC_TIMESTAMP = re.compile(
    r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}"
    r"(?:\.[0-9]{1,6})?(?:Z|\+00:00)"
)


def required(manifest: dict) -> bool:
    """Missing markers retain the legacy layout, not a presumed passing screen."""
    if "host_preflight" not in manifest:
        return False
    if manifest["host_preflight"] != {"schema": SCHEMA}:
        raise ValueError("host preflight manifest contract is unsupported")
    return True


def _unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("host preflight JSON contains duplicate keys")
        result[key] = value
    return result


def _reject_constant(_value):
    raise ValueError("host preflight JSON contains a nonfinite number")


def validate_record(raw: bytes) -> dict:
    """Check captured bytes on both receipt creation and verification.

    This validates the recorded screen, not its origin, freshness, or the host
    during later measurements. In particular, builds may follow this screen.
    """
    try:
        record = json.loads(raw, object_pairs_hook=_unique_object,
                            parse_constant=_reject_constant)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError("host preflight contains invalid JSON") from error
    if not isinstance(record, dict) or record.keys() not in (
            RECORD_FIELDS, RECORD_FIELDS | {LIMIT_FIELD}):
        raise ValueError("host preflight has unexpected record fields")
    limit = record.get(LIMIT_FIELD, DEFAULT_TOTAL_CPU_LIMIT)
    if (type(limit) is not int
            or not DEFAULT_TOTAL_CPU_LIMIT <= limit <= MAX_TOTAL_CPU_LIMIT):
        raise ValueError("host preflight total CPU limit is outside the allowed range")
    if (record["schema"] != SCHEMA
            or record["startup_screen_passed"] is not True
            or record["clean_host_performance_eligible"] is not False
            or record["limitation"] != LIMITATION):
        raise ValueError("host preflight must pass and remain startup-only")
    samples = record["samples"]
    if not isinstance(samples, list) or len(samples) != 3:
        raise ValueError("host preflight requires exactly three samples")
    previous = None
    for sample in samples:
        if not isinstance(sample, dict) or sample.keys() != SAMPLE_FIELDS:
            raise ValueError("host preflight has unexpected sample fields")
        total = sample["total_cpu_percent"]
        if (type(total) not in (int, float) or not 0 <= total < limit
                or not math.isfinite(total)
                or sample["busy_processes"] != []
                or sample["startup_screen_passed"] is not True):
            raise ValueError("host preflight sample did not pass the CPU screen")
        timestamp = sample["observed_at"]
        if not isinstance(timestamp, str) or UTC_TIMESTAMP.fullmatch(timestamp) is None:
            raise ValueError("host preflight sample requires a UTC timestamp")
        try:
            observed = datetime.fromisoformat(timestamp.replace("Z", "+00:00"))
        except ValueError as error:
            raise ValueError("host preflight sample has an invalid timestamp") from error
        if observed.utcoffset() != timedelta(0) or (previous and observed <= previous):
            raise ValueError("host preflight timestamps must be increasing UTC times")
        previous = observed
    return record
