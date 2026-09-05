#!/usr/bin/env python3
"""Host-screen contract mutations, receipt binding, and legacy compatibility."""

import argparse
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

import host_evidence as HOST


sys.dont_write_bytecode = True
DIRECTORY = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location(
    "matrix_host_fixtures", DIRECTORY / "test-matrix-publication.py")
FIXTURES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(FIXTURES)
PUBLICATION = FIXTURES.PUBLICATION


class HostRecordTests(unittest.TestCase):
    def test_contract_is_explicit_and_versioned(self):
        self.assertFalse(HOST.required({}))
        self.assertTrue(HOST.required({"host_preflight": {"schema": HOST.SCHEMA}}))
        for value in (None, False, {}, HOST.SCHEMA, {"schema": "v2"},
                      {"schema": HOST.SCHEMA, "required": False},
                      {"schema": HOST.SCHEMA, "path": "elsewhere.json"}):
            with self.subTest(value=value), self.assertRaises(ValueError):
                HOST.required({"host_preflight": value})

    def test_valid_record_stays_startup_only(self):
        record = FIXTURES.passing_host_preflight()
        self.assertEqual(HOST.validate_record(json.dumps(record).encode()), record)
        for sample in record["samples"]:
            sample["observed_at"] = sample["observed_at"].replace("+00:00", "Z")
        self.assertEqual(HOST.validate_record(json.dumps(record).encode()), record)
        self.assertIs(record["clean_host_performance_eligible"], False)

    def test_explicit_limit_is_recorded_and_bounded(self):
        record = FIXTURES.passing_host_preflight()
        record["total_cpu_limit_percent"] = 400
        record["samples"][1]["total_cpu_percent"] = 350.25
        self.assertEqual(HOST.validate_record(json.dumps(record).encode()), record)
        record["samples"][1]["total_cpu_percent"] = 400
        with self.assertRaisesRegex(ValueError, "CPU screen"):
            HOST.validate_record(json.dumps(record).encode())
        for value in (199, 401, 300.0, "300", None, True):
            record = FIXTURES.passing_host_preflight()
            record["total_cpu_limit_percent"] = value
            with self.subTest(limit=value), self.assertRaises(ValueError):
                HOST.validate_record(json.dumps(record).encode())
        legacy = FIXTURES.passing_host_preflight()
        legacy["samples"][2]["total_cpu_percent"] = 250
        with self.assertRaisesRegex(ValueError, "CPU screen"):
            HOST.validate_record(json.dumps(legacy).encode())

    def test_record_mutations_fail_closed(self):
        mutations = {
            "schema": [None, "v2"], "startup_screen_passed": [False, 1, None],
            "clean_host_performance_eligible": [True, 0, None],
            "limitation": [None, "clean host"],
            "samples": [None, [], [{}, {}], [{}, {}, {}, {}]],
            "error": ["inventory unavailable"], "unknown": [True],
        }
        for key, values in mutations.items():
            for value in values:
                record = FIXTURES.passing_host_preflight()
                record[key] = value
                with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                    HOST.validate_record(json.dumps(record).encode())
        for key in HOST.RECORD_FIELDS:
            record = FIXTURES.passing_host_preflight()
            del record[key]
            with self.subTest(missing=key), self.assertRaises(ValueError):
                HOST.validate_record(json.dumps(record).encode())

    def test_sample_mutations_fail_closed(self):
        mutations = {
            "total_cpu_percent": [-1, 200, 1000, True, "20", None,
                                  float("nan"), float("inf"), 10 ** 400],
            "busy_processes": [None, {}, [{"cpu_percent": 100}]],
            "startup_screen_passed": [False, 1, None],
            "observed_at": [None, "2026-09-05", "2026-09-05T12:00:00",
                            "2026-09-05T12:00:00-01:00", "2026-09-31T12:00:00Z",
                            "2026-09-05T12:00:00+00:00"],
            "unknown": [True],
        }
        for key, values in mutations.items():
            for value in values:
                record = FIXTURES.passing_host_preflight()
                record["samples"][1][key] = value
                with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                    HOST.validate_record(json.dumps(record).encode())
        for key in HOST.SAMPLE_FIELDS:
            record = FIXTURES.passing_host_preflight()
            del record["samples"][0][key]
            with self.subTest(missing=key), self.assertRaises(ValueError):
                HOST.validate_record(json.dumps(record).encode())

    def test_invalid_json_duplicate_keys_and_overflow_fail(self):
        good = json.dumps(FIXTURES.passing_host_preflight()).encode()
        mutations = [b"\xff", b"{", b"[]", b"null", good + good,
                     good.replace(b'"schema":', b'"schema":"duplicate","schema":'),
                     good.replace(b'"total_cpu_percent":',
                                  b'"total_cpu_percent":0,"total_cpu_percent":'),
                     good.replace(b"20.5", b"1e999"),
                     good.replace(b"20.5", b"-Infinity")]
        for raw in mutations:
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                HOST.validate_record(raw)


class HostReceiptTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="grust-host-receipt.")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.repository, self.revision = FIXTURES.make_repository(self.root)
        self.output = FIXTURES.make_bundle(self.root, self.revision)

    def issue(self):
        args = argparse.Namespace(revision=self.revision, repository=self.repository,
                                  output_dir=self.output, scale="example")
        with mock.patch.object(PUBLICATION, "run_semantic_validators"):
            PUBLICATION.issue_receipt(args, DIRECTORY)

    def test_new_receipt_binds_startup_record(self):
        self.issue()
        PUBLICATION.verify_receipt(self.output, DIRECTORY)
        receipt = json.loads((self.output / PUBLICATION.RECEIPT_NAME).read_bytes())
        raw = (self.output / HOST.FILENAME).read_bytes()
        self.assertEqual(receipt["artifact_sha256"][HOST.FILENAME], PUBLICATION.sha256(raw))
        self.assertIn({"path": HOST.FILENAME, "bytes": len(raw),
                       "sha256": PUBLICATION.sha256(raw)}, receipt["output_inventory"])

    def test_missing_record_fails_creation(self):
        (self.output / HOST.FILENAME).unlink()
        with self.assertRaisesRegex(PUBLICATION.PublicationError, "file set mismatch"):
            self.issue()

    def test_symlink_record_fails_creation(self):
        target = self.output / HOST.FILENAME
        target.rename(self.root / "screen.json")
        target.symlink_to(self.root / "screen.json")
        with self.assertRaisesRegex(PUBLICATION.PublicationError, "symlink"):
            self.issue()

    def test_failed_record_fails_creation(self):
        record = FIXTURES.passing_host_preflight()
        record["samples"][1]["total_cpu_percent"] = 800
        FIXTURES.write_json(self.output / HOST.FILENAME, record)
        with self.assertRaisesRegex(PUBLICATION.PublicationError, "CPU screen"):
            self.issue()

    def test_changed_passing_bytes_fail_verification(self):
        self.issue()
        record = FIXTURES.passing_host_preflight()
        record["samples"][1]["total_cpu_percent"] = 25
        FIXTURES.write_json(self.output / HOST.FILENAME, record)
        with self.assertRaisesRegex(PUBLICATION.PublicationError, "output inventory"):
            PUBLICATION.verify_receipt(self.output, DIRECTORY)

    def test_rehashed_failed_record_still_fails_verification(self):
        self.issue()
        record = FIXTURES.passing_host_preflight()
        record["samples"][1]["total_cpu_percent"] = 800
        raw = PUBLICATION.canonical_json(record)
        (self.output / HOST.FILENAME).write_bytes(raw)
        receipt_path = self.output / PUBLICATION.RECEIPT_NAME
        receipt = json.loads(receipt_path.read_bytes())
        receipt["artifact_sha256"][HOST.FILENAME] = PUBLICATION.sha256(raw)
        for entry in receipt["output_inventory"]:
            if entry["path"] == HOST.FILENAME:
                entry.update(bytes=len(raw), sha256=PUBLICATION.sha256(raw))
        receipt["output_bytes"] = sum(entry["bytes"] for entry in receipt["output_inventory"])
        receipt_path.write_bytes(PUBLICATION.canonical_json(receipt))
        with self.assertRaisesRegex(PUBLICATION.PublicationError, "CPU screen"):
            PUBLICATION.verify_receipt(self.output, DIRECTORY)

    def test_legacy_layout_verifies_without_host_claim(self):
        manifest = json.loads((DIRECTORY / PUBLICATION.MANIFEST_NAME).read_bytes())
        del manifest["host_preflight"]
        FIXTURES.write_json(self.output / PUBLICATION.MANIFEST_NAME, manifest)
        (self.output / HOST.FILENAME).unlink()
        receipt, _ = PUBLICATION.inspect_bundle(
            DIRECTORY, self.output, self.revision, "example", False, semantic=False)
        self.assertEqual(receipt["output_file_count"], 102)
        self.assertEqual(len(receipt["artifact_sha256"]), 54)
        self.assertNotIn(HOST.FILENAME, receipt["artifact_sha256"])
        receipt_path = self.output / PUBLICATION.RECEIPT_NAME
        raw = PUBLICATION.canonical_json(receipt)
        receipt_path.write_bytes(raw)
        PUBLICATION.verify_receipt(self.output, DIRECTORY)
        self.assertEqual(receipt_path.read_bytes(), raw)
        FIXTURES.write_json(self.output / HOST.FILENAME, FIXTURES.passing_host_preflight())
        with self.assertRaisesRegex(PUBLICATION.PublicationError, "file set mismatch"):
            PUBLICATION.verify_receipt(self.output, DIRECTORY)


if __name__ == "__main__":
    unittest.main()
