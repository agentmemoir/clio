"""Shared configuration-fingerprint shape for the clio-bench adapters.

Every report carries, per system, the settings that change that system's
behaviour. A fingerprint is captured before any probe runs and stamped into the
report, so a reader can tell which configuration produced the numbers. Each
adapter owns how its system's settings are read; this module owns the shape
every adapter returns and the minimum check the runner applies to it.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass

from scripts.extract_quality import redact_json

READ = "read"
UNREADABLE = "unreadable"


def _report_value(value: object, key: str = "") -> object:
    """Redact one fingerprint value for a report.

    A key name alone never masks a non-string value: a fingerprint read can
    carry a configuration number under a secret-shaped key (a ranking noise
    weight named `secret`), and erasing it would hide the configuration the
    report exists to record. Credential-shaped text is still masked wherever
    it appears: a string is masked when its own key is secret-shaped or its
    text matches a secret value pattern, containers are walked so their
    strings get the same rule, and every non-string value is kept.
    """
    if isinstance(value, str):
        return redact_json(value, key)
    if isinstance(value, Mapping):
        return {str(k): _report_value(v, str(k)) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [_report_value(item) for item in value]
    if value is None or isinstance(value, (bool, int, float)):
        return value
    return redact_json(value)


class FingerprintError(RuntimeError):
    """Raised when a system's configuration could not be fingerprinted."""


@dataclass(frozen=True)
class SettingRecord:
    """One behaviour-changing setting: read with its value, or unreadable.

    An unreadable record names the surfaces and commands that were tried, so a
    field with no read surface is an explicit absence rather than an empty
    object that would read as "default".
    """

    status: str
    value: object = None
    surface: str = ""
    tried: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        if self.status == READ:
            if not self.surface:
                raise ValueError("a read setting must name the surface that returned it")
        elif self.status == UNREADABLE:
            if not self.tried:
                raise ValueError("an unreadable setting must name what was tried")
        else:
            raise ValueError(f"unknown setting status {self.status!r}")

    def to_json(self) -> dict:
        """The report form of one setting, redacted before it leaves.

        Every captured value passes the report redactor on its way out:
        credential-shaped strings are masked wherever they appear, while a
        non-string value is kept even under a secret-shaped key, because that
        is configuration, not a credential.
        """
        return _report_value(self._raw_json())

    def _raw_json(self) -> dict:
        if self.status == READ:
            return {
                "status": self.status,
                "value": self.value,
                "surface": self.surface,
            }
        return {"status": self.status, "tried": list(self.tried)}


def read_setting(*, value: object, surface: str) -> SettingRecord:
    """A setting read through `surface`, carrying its effective `value`."""
    return SettingRecord(status=READ, value=value, surface=surface)


def unreadable_setting(*, tried: tuple[str, ...]) -> SettingRecord:
    """A setting with no read surface, naming the commands that were tried."""
    return SettingRecord(status=UNREADABLE, tried=tried)


@dataclass(frozen=True)
class Fingerprint:
    """One system's capture: `{system, version, settings}`.

    `required` names the settings every run of this system must read; a run
    whose fingerprint leaves one of them unreadable fails. A system with no
    readable settings surface still records one explicit unreadable setting.
    """

    system: str
    version: str
    settings: dict[str, SettingRecord]
    required: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        if not self.system:
            raise ValueError("a fingerprint must name its system")
        if not self.version:
            raise ValueError("a fingerprint must carry a version or 'unreadable'")
        if not self.settings:
            raise ValueError("a fingerprint must record at least one setting or absence")
        missing = [name for name in self.required if name not in self.settings]
        if missing:
            raise ValueError(f"required settings are absent: {missing}")

    def validate(self) -> None:
        """Raise `FingerprintError` unless every required setting was read."""
        unreadable = [
            name
            for name in self.required
            if self.settings[name].status != READ
        ]
        if unreadable:
            raise FingerprintError(
                f"{self.system}: required settings are not readable: {unreadable}"
            )

    def to_json(self) -> dict:
        """The report form of one capture, redacted as a whole."""
        return _report_value(self._raw_json())

    def _raw_json(self) -> dict:
        return {
            "system": self.system,
            "version": self.version,
            "settings": {
                name: record.to_json() for name, record in self.settings.items()
            },
        }
