"""Tests for Mnemosyne environment overrides against effective settings."""

from __future__ import annotations

import unittest
from unittest.mock import patch

from benchmarks.clio_bench.adapters.mnemosyne import MnemosyneAdapter
from benchmarks.clio_bench.fingerprint import (
    Fingerprint,
    FingerprintError,
    SettingRecord,
    read_setting,
)
from benchmarks.clio_bench.tokenizer import get_tokenizer


class FingerprintEnvOverrideTests(unittest.TestCase):
    def _adapter(self, extra_env: dict[str, str]) -> MnemosyneAdapter:
        return MnemosyneAdapter(
            binary="unused",
            data_dir="/scratch/mnemosyne",
            tokenizer=get_tokenizer(),
            extra_env=extra_env,
        )

    def _fingerprint(
        self, adapter: MnemosyneAdapter, effective_vec_weight: float
    ) -> Fingerprint:
        def effective_setting(key: str) -> SettingRecord:
            value = effective_vec_weight if key == "vec_weight" else 1
            return read_setting(value=value, surface=f"mnemosyne config get {key}")

        with (
            patch.object(adapter, "_read_setting", side_effect=effective_setting),
            patch.object(adapter, "_read_version", return_value="3.15.1"),
        ):
            return adapter.fingerprint()

    def test_mismatched_override_fails_with_a_fresh_data_dir_hint(self) -> None:
        adapter = self._adapter({"MNEMOSYNE_VEC_WEIGHT": "0.9"})

        with self.assertRaises(FingerprintError) as raised:
            self._fingerprint(adapter, 0.42)

        message = str(raised.exception)
        self.assertIn("config.yaml", message)
        self.assertIn("MNEMOSYNE_VEC_WEIGHT", message)
        self.assertIn("fresh data directory", message)

    def test_matching_override_is_kept_in_the_fingerprint(self) -> None:
        adapter = self._adapter({"MNEMOSYNE_VEC_WEIGHT": "0.42"})

        fingerprint = self._fingerprint(adapter, 0.42)

        self.assertEqual(fingerprint.settings["vec_weight"].value, 0.42)

    def test_unrequested_effective_value_is_not_rejected(self) -> None:
        adapter = self._adapter({"CLIO_BENCH_STORE": "/scratch/store"})

        fingerprint = self._fingerprint(adapter, 0.42)

        self.assertEqual(fingerprint.settings["vec_weight"].value, 0.42)


if __name__ == "__main__":
    unittest.main()
