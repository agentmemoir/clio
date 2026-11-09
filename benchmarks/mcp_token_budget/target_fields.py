#!/usr/bin/env python3
"""Shared field checks for the frozen preset target record.

Every refusal is one line prefixed ``targets:`` and names the rule it broke, so
the target record and its adjustments fail in the same language. This module is
imported by ``budget_targets`` and ``target_adjustment`` and holds no record of
its own.
"""

import re


class TargetError(ValueError):
    """Raised when a preset target record cannot be trusted."""


def _fail(message):
    raise TargetError(f"targets: {message}")


def _is_int(value) -> bool:
    """Return true for a JSON integer and false for a boolean."""
    return isinstance(value, int) and not isinstance(value, bool)


def _key_message(where, keys, allowed) -> str:
    missing = sorted(allowed - keys)
    unexpected = sorted(keys - allowed)
    parts = []
    if missing:
        parts.append(f"missing {missing}")
    if unexpected:
        parts.append(f"unexpected {unexpected}")
    return f"{where} keys must be exactly {sorted(allowed)}: " + ", ".join(parts)


def _object(value, where, allowed) -> dict:
    """Return ``value`` as an object with exactly the ``allowed`` keys."""
    if not isinstance(value, dict):
        _fail(f"{where} must be an object, got {type(value).__name__}")
    keys = set(value)
    if keys != allowed:
        _fail(_key_message(where, keys, allowed))
    return value


def _nonempty(value, where) -> str:
    """Return ``value`` as a non-empty string, or refuse it."""
    if not isinstance(value, str) or not value.strip():
        _fail(f"{where} must be a nonempty string, got {value!r}")
    return value


def _expect(value, where, expected) -> None:
    """Refuse ``value`` unless it is exactly ``expected``."""
    if value != expected or type(value) is not type(expected):
        _fail(f"{where} is {value!r}, expected {expected!r}")


def _repo_relative(path, where) -> str:
    """Refuse a path that is absolute, climbs out, or names a private segment."""
    _nonempty(path, where)
    segments = re.split(r"[/\\]", path)
    refusals = (
        (path.startswith(("/", "\\")), "a leading slash or backslash"),
        (bool(re.match(r"^[A-Za-z]:", path)), "a drive letter"),
        (bool(re.match(r"^[A-Za-z][A-Za-z0-9+.-]*:", path)), "a URI scheme"),
        (".." in segments, "a '..' segment"),
        ("private" in segments, "a private segment"),
    )
    for refused, what in refusals:
        if refused:
            _fail(f"{where} is {path!r}, expected a repo-relative path with no {what}")
    return path
