"""The adapter registry and public surface of the clio-bench runner.

`cli.py` reads `ADAPTER_BUILDERS` to list `--adapter` choices and `capture_fingerprints` to read a system's configuration without running a probe. The execution, selection, and suite-loading API (`run_suite`, `select_work`, `load_suite`) lives in `suite_execution`, `work_selection`, and `suite_loading`; this module re-exports those names so the historic `benchmarks.clio_bench.runner` path keeps every one.
"""

from __future__ import annotations

from argparse import Namespace
from importlib import import_module
from pathlib import Path

from .fingerprint import unreadable_setting
from .suite_execution import (
    INGEST_COUNTERS,
    AccountingError,
    EmptyStoreError,
    run_suite,
)
from .suite_loading import _DEFAULT_PINS, IMPLEMENTED_SUITES, load_suite
from .tokenizer import get_tokenizer
from .work_selection import select_work

__all__ = [
    "_DEFAULT_PINS",
    "ADAPTER_BUILDERS",
    "ADAPTER_DEFAULT_BINARIES",
    "ADAPTER_DEFAULT_DATA_DIRS",
    "ADAPTER_NAMES",
    "AccountingError",
    "EmptyStoreError",
    "IMPLEMENTED_SUITES",
    "INGEST_COUNTERS",
    "capture_fingerprints",
    "default_data_dir",
    "load_suite",
    "run_suite",
    "select_work",
]

# The executable each adapter drives when --binary is absent. One adapter owns
# one system, so a single flag cannot name the binary for whichever adapter was
# selected; each builder reads its own entry here rather than repeating the
# literal, and the contract test asserts the resolution against this map. An
# adapter that drives nothing in-process has no entry and no executable to
# default. Every key must name a registered adapter.
ADAPTER_DEFAULT_BINARIES = {
    "clio": "clio",
    "mnemosyne": "mnemosyne",
}

# The data directory each CLI-backed system reads when --data-dir is absent.
# The builders and the blocked-gate capture both resolve through this map, so
# a record written without a probe describes the same directory a scored run
# reads. Every key must name a registered adapter that takes a data directory.
ADAPTER_DEFAULT_DATA_DIRS = {
    "clio": "benchmarks/.cache/clio-data",
    "mnemosyne": "benchmarks/.cache/mnemosyne-data",
}


def default_data_dir(name: str, explicit: str | None) -> str | None:
    """Resolve one system's data directory, explicit flag over the map default.

    A system with no default (an in-process double) keeps whatever was passed,
    which is normally None because it reads nothing from disk.
    """
    default = ADAPTER_DEFAULT_DATA_DIRS.get(name)
    if default is None:
        return explicit
    return explicit or str(Path(default).resolve())


def _build_memory(tokenizer, options):
    """Build the deterministic in-process adapter."""
    from .adapters.memory import MemoryAdapter

    return MemoryAdapter(tokenizer)


def _build_clio(tokenizer, options):
    """Build the adapter that drives the real clio binary."""
    from .adapters.clio import ClioAdapter

    return ClioAdapter(
        binary=options.binary or ADAPTER_DEFAULT_BINARIES["clio"],
        data_dir=default_data_dir("clio", options.data_dir),
        tokenizer=tokenizer,
        limit=options.retrieve_limit,
    )


def _build_mnemosyne(tokenizer, options):
    """Build the adapter that drives the real mnemosyne binary."""
    from .adapters.mnemosyne import MnemosyneAdapter

    return MnemosyneAdapter(
        binary=options.binary or ADAPTER_DEFAULT_BINARIES["mnemosyne"],
        data_dir=default_data_dir("mnemosyne", options.data_dir),
        tokenizer=tokenizer,
        limit=options.retrieve_limit,
    )


# The registry. The CLI's `--adapter` choices and the adapter contract test
# both read it, so an adapter cannot be selectable without being held to the
# five-method contract, and adding one needs no edit outside this map.
ADAPTER_BUILDERS = {
    "memory": _build_memory,
    "clio": _build_clio,
    "mnemosyne": _build_mnemosyne,
}
ADAPTER_NAMES = tuple(ADAPTER_BUILDERS)


def _expected_settings(name: str) -> list[str]:
    """The settings a system's own surfaces expose, for a degraded record."""
    try:
        module = import_module(f".adapters.{name}", package=__package__)
    except ImportError:
        return []
    return list(getattr(module, "REQUIRED_SETTINGS", ()))


def capture_fingerprints(
    systems: list[str],
    *,
    binary_for,
    data_dir_for,
    retrieve_limit: int,
    pins: dict,
) -> dict[str, dict]:
    """Read each named system's fingerprint without running a probe.

    Used only for a blocked-gate record. Each system is built through the
    registry the way a scored run builds it, with the same default data
    directory, so the record describes the configuration the run would have
    used. Every block carries `fingerprint_complete`; a read that fails is an
    explicit incomplete block naming the system's expected settings, never a
    clean-looking stub.
    """
    tokenizer = get_tokenizer(pins["judge"]["tokenizer"])
    captured: dict[str, dict] = {}
    for name in systems:
        try:
            options = Namespace(
                binary=binary_for(name),
                data_dir=default_data_dir(name, data_dir_for(name)),
                retrieve_limit=retrieve_limit,
            )
            adapter = ADAPTER_BUILDERS[name](tokenizer, options)
            fingerprint = adapter.fingerprint()
            fingerprint.validate()
            block = fingerprint.to_json()
            block["fingerprint_complete"] = True
        except Exception as exc:  # noqa: BLE001 - the miss is recorded, never hidden
            block = {
                "system": name,
                "version": "unreadable",
                "fingerprint_complete": False,
                "expected_settings": _expected_settings(name),
                "settings": {
                    "fingerprint": unreadable_setting(
                        tried=(f"fingerprint read failed: {exc}",)
                    ).to_json()
                },
            }
        captured[name] = block
    return captured
