"""Unit tests for pinned snapshot fetch and verification."""

from __future__ import annotations

import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from benchmarks.clio_bench import fetch


class HashTests(unittest.TestCase):
    def test_git_blob_hash_matches_git(self) -> None:
        self.assertEqual(
            fetch.git_blob_sha1(b"hello\n"),
            "ce013625030ba8dba906f756967f9e9ca394464a",
        )

    def test_sha256_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "abc.bin"
            path.write_bytes(b"abc")
            self.assertEqual(
                fetch.sha256_file(path),
                hashlib.sha256(b"abc").hexdigest(),
            )


class VerifyTests(unittest.TestCase):
    def test_sha256_match_passes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "f.bin"
            path.write_bytes(b"abc")
            fetch.verify_file(path, {"sha256": hashlib.sha256(b"abc").hexdigest()})

    def test_sha256_mismatch_raises(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "f.bin"
            path.write_bytes(b"abc")
            with self.assertRaises(fetch.PinError):
                fetch.verify_file(path, {"sha256": "0" * 64})

    def test_git_blob_mismatch_raises(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "f.bin"
            path.write_bytes(b"hello\n")
            with self.assertRaises(fetch.PinError):
                fetch.verify_file(path, {"git_blob": "0" * 40})


class EnsureDatasetTests(unittest.TestCase):
    def test_existing_verified_file_is_reused(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cache = Path(tmp)
            path = cache / "data.json"
            path.write_bytes(b"abc")
            pin = {
                "filename": "data.json",
                "url": "http://invalid.example/data.json",
                "sha256": hashlib.sha256(b"abc").hexdigest(),
            }
            with mock.patch.object(fetch, "_download") as download:
                result = fetch.ensure_dataset(pin, cache)
            download.assert_not_called()
            self.assertEqual(result, path)

    def test_download_then_verify(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            cache = Path(tmp)
            pin = {
                "filename": "data.json",
                "url": "http://invalid.example/data.json",
                "sha256": hashlib.sha256(b"abc").hexdigest(),
            }

            def fake_download(url: str, dest: Path) -> None:
                dest.write_bytes(b"abc")

            with mock.patch.object(fetch, "_download", side_effect=fake_download):
                result = fetch.ensure_dataset(pin, cache)
            self.assertEqual(result.read_bytes(), b"abc")

    def test_pin_mismatch_after_download_raises(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pin = {
                "filename": "data.json",
                "url": "http://invalid.example/data.json",
                "sha256": "0" * 64,
            }
            with mock.patch.object(
                fetch, "_download", side_effect=lambda url, dest: dest.write_bytes(b"abc")
            ):
                with self.assertRaises(fetch.PinError):
                    fetch.ensure_dataset(pin, Path(tmp))

    def test_transport_failure_raises_missing_snapshot(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            pin = {"filename": "x.json", "url": "http://invalid.example/x.json"}
            with mock.patch.object(
                fetch, "_download", side_effect=OSError("no route")
            ):
                with self.assertRaises(fetch.MissingSnapshotError):
                    fetch.ensure_dataset(pin, Path(tmp))


class LoadPinsTests(unittest.TestCase):
    def test_loads_json_object(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "pins.json"
            path.write_text(json.dumps({"suites": {"locomo": {}}}))
            self.assertIn("suites", fetch.load_pins(path))


if __name__ == "__main__":
    unittest.main()
