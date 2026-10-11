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
        self,
        adapter: MnemosyneAdapter,
        effective_vec_weight: float,
        effective_cross_session: object = True,
        ambient_env: dict[str, str] | None = None,
    ) -> Fingerprint:
        def effective_setting(key: str) -> SettingRecord:
            if key == "vec_weight":
                value = effective_vec_weight
            elif key == "cross_session":
                value = effective_cross_session
            else:
                value = 1
            return read_setting(value=value, surface=f"mnemosyne config get {key}")

        with patch.dict("os.environ", ambient_env or {}, clear=True):
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

    def test_ambient_override_mismatch_is_rejected(self) -> None:
        adapter = self._adapter({})

        with self.assertRaises(FingerprintError) as raised:
            self._fingerprint(
                adapter,
                0.42,
                ambient_env={"MNEMOSYNE_VEC_WEIGHT": "0.9"},
            )

        self.assertIn("MNEMOSYNE_VEC_WEIGHT", str(raised.exception))

    def test_extra_env_override_takes_precedence_over_ambient_value(self) -> None:
        adapter = self._adapter({"MNEMOSYNE_VEC_WEIGHT": "0.42"})

        fingerprint = self._fingerprint(
            adapter,
            0.42,
            ambient_env={"MNEMOSYNE_VEC_WEIGHT": "0.9"},
        )

        self.assertEqual(fingerprint.settings["vec_weight"].value, 0.42)

    def test_cross_session_boolean_aliases_match_effective_value(self) -> None:
        cases = (("YES", True), ("on", True), ("NO", False), ("off", False))

        for requested, effective in cases:
            with self.subTest(requested=requested):
                adapter = self._adapter({"MNEMOSYNE_CROSS_SESSION": requested})

                fingerprint = self._fingerprint(adapter, 0.42, effective)

                self.assertIs(
                    fingerprint.settings["cross_session"].value,
                    effective,
                )

    def test_string_boolean_from_config_matches_environment_alias(self) -> None:
        cases = (("on", "yes", True), ("off", "no", False))

        for requested, effective, expected in cases:
            with self.subTest(requested=requested, effective=effective):
                adapter = self._adapter({"MNEMOSYNE_CROSS_SESSION": requested})

                fingerprint = self._fingerprint(adapter, 0.42, effective)

                self.assertIs(fingerprint.settings["cross_session"].value, expected)


if __name__ == "__main__":
    unittest.main()
