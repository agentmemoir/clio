"""Unit tests for the shared configuration-fingerprint shape."""

from __future__ import annotations

import unittest

from benchmarks.clio_bench.fingerprint import (
    Fingerprint,
    FingerprintError,
    SettingRecord,
    read_setting,
    unreadable_setting,
)


class SettingRecordTests(unittest.TestCase):
    def test_a_read_setting_carries_its_value_and_surface(self) -> None:
        record = read_setting(value={"verbosity": "balanced"}, surface="cli get")
        self.assertEqual(
            record.to_json(),
            {
                "status": "read",
                "value": {"verbosity": "balanced"},
                "surface": "cli get",
            },
        )

    def test_an_unreadable_setting_names_what_was_tried(self) -> None:
        record = unreadable_setting(tried=("probe -> no such field",))
        self.assertEqual(
            record.to_json(),
            {"status": "unreadable", "tried": ["probe -> no such field"]},
        )

    def test_a_read_setting_must_name_its_surface(self) -> None:
        with self.assertRaises(ValueError):
            SettingRecord(status="read", value=1)

    def test_an_unreadable_setting_must_name_what_was_tried(self) -> None:
        with self.assertRaises(ValueError):
            SettingRecord(status="unreadable")

    def test_an_unknown_status_is_rejected(self) -> None:
        with self.assertRaises(ValueError):
            SettingRecord(status="broken", tried=("x",))

    def test_a_secret_key_inside_a_value_is_masked(self) -> None:
        record = read_setting(
            value={
                "dense": 0.25,
                "embedding_api_key": "sk-abcdef1234567890",
            },
            surface="cli ranking get",
        )
        payload = record.to_json()
        self.assertEqual(payload["value"]["embedding_api_key"], "[REDACTED]")
        self.assertEqual(payload["value"]["dense"], 0.25)

    def test_a_non_string_value_under_a_secret_key_is_kept(self) -> None:
        record = read_setting(
            value={"hygiene": {"secret": 0.35}, "dense": 0.25},
            surface="cli ranking get",
        )
        payload = record.to_json()
        self.assertEqual(payload["value"]["hygiene"]["secret"], 0.35)
        self.assertEqual(payload["value"]["dense"], 0.25)

    def test_a_secret_shaped_string_value_is_masked(self) -> None:
        record = read_setting(
            value="bearer abcdefgh12345678",
            surface="cli ranking get",
        )
        self.assertEqual(record.to_json()["value"], "Bearer [REDACTED]")


class FingerprintTests(unittest.TestCase):
    def _valid(self, **overrides) -> Fingerprint:
        payload = {
            "system": "clio",
            "version": "1.2.3",
            "settings": {
                "retention_profile": read_setting(value={}, surface="cli get")
            },
            "required": ("retention_profile",),
        }
        payload.update(overrides)
        return Fingerprint(**payload)

    def test_to_json_is_the_documented_shape(self) -> None:
        payload = self._valid().to_json()
        self.assertEqual(set(payload), {"system", "version", "settings"})
        self.assertEqual(payload["system"], "clio")
        self.assertEqual(payload["version"], "1.2.3")
        self.assertEqual(
            payload["settings"]["retention_profile"]["status"], "read"
        )

    def test_a_secret_in_any_field_is_masked_in_the_report_shape(self) -> None:
        fingerprint = self._valid(
            settings={
                "ranking_environment": read_setting(
                    value={"api_key": "sk-abcdef1234567890"},
                    surface="cli ranking get",
                )
            },
            required=("ranking_environment",),
        )
        value = fingerprint.to_json()["settings"]["ranking_environment"]["value"]
        self.assertEqual(value["api_key"], "[REDACTED]")

    def test_validate_accepts_a_complete_fingerprint(self) -> None:
        self._valid().validate()

    def test_validate_rejects_an_unreadable_required_setting(self) -> None:
        fingerprint = Fingerprint(
            system="clio",
            version="1.2.3",
            settings={
                "retention_profile": unreadable_setting(tried=("cli probe",))
            },
            required=("retention_profile",),
        )
        with self.assertRaises(FingerprintError):
            fingerprint.validate()

    def test_an_empty_settings_mapping_is_rejected(self) -> None:
        with self.assertRaises(ValueError):
            Fingerprint(system="clio", version="1.2.3", settings={})

    def test_a_required_setting_that_is_absent_is_rejected(self) -> None:
        with self.assertRaises(ValueError):
            Fingerprint(
                system="clio",
                version="1.2.3",
                settings={"a": read_setting(value=1, surface="s")},
                required=("b",),
            )

    def test_an_unnamed_system_is_rejected(self) -> None:
        with self.assertRaises(ValueError):
            Fingerprint(
                system="",
                version="1.2.3",
                settings={"a": read_setting(value=1, surface="s")},
            )

    def test_an_empty_version_is_rejected(self) -> None:
        with self.assertRaises(ValueError):
            Fingerprint(
                system="clio",
                version="",
                settings={"a": read_setting(value=1, surface="s")},
            )


if __name__ == "__main__":
    unittest.main()
