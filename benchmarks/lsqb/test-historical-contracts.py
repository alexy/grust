#!/usr/bin/env python3
"""Fixture-free historical contract and SDK selection regression tests."""

from contextlib import contextmanager
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import historical_contracts as contracts


ROOT = Path(__file__).resolve().parent
DIGEST = "1dcae942840f216a83282f45f27e7fe228616e8f51af764689dc4f4fea0de849"
PROFILES = (
    ("surreal-sdk", "validate-sdk.py", "945dfa748b99e259f21973a1e86aaa339a028834"),
    ("helix-sdk", "validate-helix-sdk.py", "ed3febd88d35c5a6bd6c090787536dc0f33c85cd"),
)


def load_auditor(filename):
    spec = importlib.util.spec_from_file_location(filename.removesuffix(".py"), ROOT / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class HistoricalContractTests(unittest.TestCase):
    def setUp(self):
        self.raw = (contracts.CONTRACT_DIRECTORY / f"{DIGEST}.json").read_bytes()

    @contextmanager
    def temporary_contract(self, raw=None, digest=DIGEST):
        with tempfile.TemporaryDirectory(prefix="grust-historical-contract.") as temporary:
            directory = Path(temporary)
            path = directory / f"{digest}.json"
            if raw is not None:
                path.write_bytes(raw)
            with mock.patch.object(contracts, "CONTRACT_DIRECTORY", directory):
                yield path

    def test_default_contract_bytes_and_legacy_semantics(self):
        self.assertEqual(len(self.raw), 21840)
        self.assertEqual(hashlib.sha256(self.raw).hexdigest(), DIGEST)
        manifest = contracts.load_manifest(DIGEST)
        self.assertEqual(manifest, json.loads(self.raw))
        self.assertNotIn("host_preflight", manifest)
        self.assertNotIn("execution_plans", manifest)
        self.assertEqual(manifest["tracks"]["baseline"]["query_order"],
                         [f"q{index}" for index in range(1, 10)])
        self.assertEqual(len(manifest["tracks"]["adversarial"]["query_order"]), 13)

    def test_unknown_digest_is_rejected_before_reading(self):
        for digest in (None, True, [], {}, "", "f" * 64, DIGEST.upper(), "../" + DIGEST):
            with self.subTest(digest=digest), mock.patch.object(contracts, "_read_regular") as read:
                with self.assertRaisesRegex(ValueError, "unknown historical contract"):
                    contracts.load_manifest(digest)
                read.assert_not_called()

    def test_current_contract_and_byte_or_whitespace_mutations_are_rejected(self):
        current = (ROOT / "evidence-manifest-v2.json").read_bytes()
        self.assertNotEqual(hashlib.sha256(current).hexdigest(), DIGEST)
        for raw in (current, self.raw + b"\n", self.raw.replace(b"v2", b"v9", 1)):
            with self.subTest(length=len(raw)), self.temporary_contract(raw):
                with mock.patch.object(contracts.json, "loads") as parse:
                    with self.assertRaisesRegex(ValueError, "digest mismatch"):
                        contracts.load_manifest(DIGEST)
                    parse.assert_not_called()

    def test_missing_file_has_no_fallback(self):
        with self.temporary_contract():
            with self.assertRaisesRegex(ValueError, "unavailable"):
                contracts.load_manifest(DIGEST)

    def test_symlinks_are_rejected_even_with_authentic_target_bytes(self):
        with self.temporary_contract() as path:
            target = path.with_name("authentic.json")
            target.write_bytes(self.raw)
            path.symlink_to(target)
            with self.assertRaisesRegex(ValueError, "regular non-symlink"):
                contracts.load_manifest(DIGEST)
            target.unlink()
            with self.assertRaisesRegex(ValueError, "regular non-symlink"):
                contracts.load_manifest(DIGEST)

    def test_directory_is_not_a_contract_file(self):
        with self.temporary_contract() as path:
            path.mkdir()
            with self.assertRaisesRegex(ValueError, "regular non-symlink"):
                contracts.load_manifest(DIGEST)

    @unittest.skipUnless(hasattr(os, "mkfifo"), "FIFO files require POSIX")
    def test_fifo_is_rejected_without_opening_it(self):
        with self.temporary_contract() as path:
            os.mkfifo(path)
            with mock.patch.object(contracts.os, "open") as opened:
                with self.assertRaisesRegex(ValueError, "regular non-symlink"):
                    contracts.load_manifest(DIGEST)
                opened.assert_not_called()

    def test_symlinked_contract_directory_is_rejected(self):
        with self.temporary_contract(self.raw) as path:
            link = path.parent / "redirect"
            link.symlink_to(path.parent, target_is_directory=True)
            with mock.patch.object(contracts, "CONTRACT_DIRECTORY", link):
                with self.assertRaisesRegex(ValueError, "directory must not be a symlink"):
                    contracts.load_manifest(DIGEST)

    def test_replacement_between_stat_and_open_is_rejected(self):
        with self.temporary_contract(self.raw) as path:
            real_open = os.open

            def replace_before_open(filename, flags):
                path.rename(path.with_name("previous.json"))
                path.write_bytes(self.raw)
                return real_open(filename, flags)

            with mock.patch.object(contracts.os, "open", side_effect=replace_before_open):
                with self.assertRaisesRegex(ValueError, "changed while it was opened"):
                    contracts.load_manifest(DIGEST)

    def test_parser_stays_strict_for_any_future_allowlisted_contract(self):
        invalid = (b"{", b"[]", b"null", b"\xff", b"{}",
                   b'{"schema":"grust-lsqb-evidence-manifest-v2","schema":"duplicate"}',
                   b'{"schema":"grust-lsqb-evidence-manifest-v2","value":NaN}',
                   b'{"schema":"grust-lsqb-evidence-manifest-v2","nested":{"x":1,"x":2}}')
        for raw in invalid:
            digest = hashlib.sha256(raw).hexdigest()
            with self.subTest(raw=raw), self.temporary_contract(raw, digest):
                with mock.patch.object(contracts, "KNOWN_MANIFESTS", frozenset({digest})):
                    with self.assertRaises(ValueError):
                        contracts.load_manifest(digest)

    def test_both_source_profiles_keep_their_original_contract(self):
        for backend, filename, source in PROFILES:
            auditor = load_auditor(filename)
            with self.subTest(backend=backend):
                self.assertEqual(auditor.SOURCE, source)
                self.assertEqual(auditor.MANIFEST, DIGEST)
                self.assertEqual(auditor.load_manifest(auditor.MANIFEST), json.loads(self.raw))

    def test_auditors_do_not_consult_the_mutable_current_manifest(self):
        for backend, filename, _source in PROFILES:
            auditor = load_auditor(filename)
            with self.subTest(backend=backend), tempfile.TemporaryDirectory() as temporary:
                directory = Path(temporary)
                (directory / "evidence-manifest-v2.json").write_bytes(b"not the pinned contract")
                with mock.patch.object(auditor, "ROOT", directory):
                    with mock.patch.object(auditor.matrix, "load_json",
                                           side_effect=ValueError("reached evidence loading")):
                        with mock.patch.object(auditor, "load_manifest",
                                               wraps=contracts.load_manifest) as selected:
                            with self.assertRaisesRegex(ValueError, "reached evidence loading"):
                                auditor.validate(directory)
                            selected.assert_called_once_with(DIGEST)

    def test_unknown_source_still_fails_after_historical_contract_selection(self):
        timing = dict(warmup_iterations=2, measurement_iterations=10,
                      query_timeout_ms=60000, worker_ready_timeout_ms=30000,
                      query_reap_grace_ms=250, query_kill_reap_timeout_ms=5000,
                      query_recovery_timeout_ms=15000, cell_timeout_ms=17799000,
                      timeout_enforcement="coordinator-process-group", query_order="rotating",
                      boundary="coordinator-go-to-result-consumed")
        for backend, filename, _source in PROFILES:
            auditor = load_auditor(filename)
            records = {
                "component.json": dict(schema_version=3, valid=True, complete=False,
                                       warning="These are not LDBC Benchmark Results.", timing=timing),
                "invocation.json": dict(publication_qualified=False, source_revision="f" * 40,
                                        client_image_id=auditor.CLIENT, backend=backend),
            }

            def read(path, _label):
                return records[path.name], b""

            with self.subTest(backend=backend), mock.patch.object(auditor.matrix, "load_json", side_effect=read):
                with mock.patch.object(auditor.matrix, "validate_v3_timeout_contract"):
                    with self.assertRaisesRegex(auditor.matrix.PublicationError, "invocation identity differs"):
                        auditor.validate(Path("unused"))


if __name__ == "__main__":
    unittest.main()
