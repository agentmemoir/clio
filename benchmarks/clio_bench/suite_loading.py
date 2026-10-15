"""Resolving and verifying one pinned suite snapshot.

`load_suite` turns a suite name and a pin into the parsed `SuiteData` the runner drives.
"""

from __future__ import annotations

from importlib import import_module
from pathlib import Path

from .stubs import StubSuiteError, require_implemented
from .types import SuiteData

# The suites that can be scored. `cli.py` reads it to expand `--suite both` and
# to list the choices, so it is part of this module's surface rather than a
# private detail of `load_suite`.
IMPLEMENTED_SUITES = ("locomo", "longmemeval")
_DEFAULT_PINS = Path(__file__).resolve().parent / "pins.json"


def _dataset_module(suite: str):
    return import_module(f".datasets.{suite}", package=__package__)


def load_suite(suite: str, pins: dict, cache_dir: Path) -> SuiteData:
    """Resolve, verify, and parse a suite snapshot."""
    require_implemented(suite)
    if suite not in IMPLEMENTED_SUITES:
        raise StubSuiteError(
            f"unknown suite {suite!r}; implemented suites: {list(IMPLEMENTED_SUITES)}"
        )
    pin = pins["suites"][suite]
    return _dataset_module(suite).load(pin, cache_dir)
