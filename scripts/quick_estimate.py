"""Estimate a Quick run from the per-test times nextest recorded last time.

The estimate is the sum of past times for the tests a selection matches, read
from the newest JUnit files. It is a plan, not a promise: a test that has never
run has no time, and build time is not in it. With no history it says so.
"""

from __future__ import annotations

import xml.etree.ElementTree as ET
from pathlib import Path
from typing import Callable, Iterable, NamedTuple


class Case(NamedTuple):
    binary_id: str
    name: str
    seconds: float
    failed: bool


class Estimate(NamedTuple):
    seconds: float
    tests: int


def read_junit(path: Path) -> list[Case]:
    """The test cases of one JUnit file; an unreadable file has none."""
    try:
        root = ET.parse(path).getroot()
    except (OSError, ET.ParseError):
        return []
    cases = []
    for case in root.iter("testcase"):
        try:
            seconds = float(case.get("time", "0"))
        except ValueError:
            continue
        failed = case.find("failure") is not None or case.find("error") is not None
        cases.append(Case(case.get("classname", ""), case.get("name", ""), seconds, failed))
    return cases


def newest_history(paths: Iterable[Path]) -> list[Case]:
    """One case per (binary id, name); the newest file wins, by modified time."""
    existing = [path for path in paths if path.is_file()]
    merged: dict[tuple[str, str], Case] = {}
    for path in sorted(existing, key=lambda p: p.stat().st_mtime):
        for case in read_junit(path):
            merged[(case.binary_id, case.name)] = case
    return list(merged.values())


def estimate(history: Iterable[Case], keep: Callable[[Case], bool]) -> Estimate | None:
    """Summed time of the history cases `keep` accepts; None without a match."""
    picked = [case for case in history if keep(case)]
    if not picked:
        return None
    return Estimate(sum(case.seconds for case in picked), len(picked))


def failed_cases(path: Path) -> list[tuple[str, str]]:
    """The (binary id, name) pairs that failed in one JUnit file."""
    return [(case.binary_id, case.name) for case in read_junit(path) if case.failed]


def wall_seconds(summed: float, threads: int) -> float:
    """Test time at `threads` workers, ignoring build time and scheduling gaps."""
    return summed / max(threads, 1)
