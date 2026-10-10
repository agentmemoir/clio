"""Documented stubs for the three suites that are not implemented.

BEAM, MemoryAgentBench, and AMA-Bench are intentionally out of scope for this
build. They are recorded here as explicit, machine-readable stubs so the
runner can refuse them with a clear message and no score is ever fabricated.
"""

from __future__ import annotations

STUB_SUITES: dict[str, dict] = {
    "beam": {
        "name": "BEAM (Agent Memory Benchmark)",
        "implemented": False,
        "reason": "Not implemented in this build; the two first suites ship first.",
        "reference": "Massive-scale memory evaluation across 128K-10M token tiers.",
        "owner": "A later phase extends the same adapter interface to BEAM.",
    },
    "memoryagentbench": {
        "name": "MemoryAgentBench",
        "implemented": False,
        "reason": "Not implemented in this build; the two first suites ship first.",
        "reference": "Incremental inject-once/query-many streaming evaluation.",
        "owner": "A later phase extends the same adapter interface to MemoryAgentBench.",
    },
    "ama-bench": {
        "name": "AMA-Bench",
        "implemented": False,
        "reason": "Not implemented in this build; the two first suites ship first.",
        "reference": "Long-horizon agent execution-trajectory memory QA.",
        "owner": "A later phase extends the same adapter interface to AMA-Bench.",
    },
}


class StubSuiteError(RuntimeError):
    """Raised when a caller tries to run a suite that is only a stub."""


def require_implemented(suite: str) -> None:
    """Fail clearly when `suite` is one of the documented stubs."""
    if suite in STUB_SUITES:
        stub = STUB_SUITES[suite]
        raise StubSuiteError(
            f"{stub['name']} is a documented stub and is not implemented: "
            f"{stub['reason']}"
        )