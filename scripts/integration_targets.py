#!/usr/bin/env python3
"""Integration test targets: which Cargo test target a file under tests/ builds into.

Cargo compiles every `tests/<name>.rs` as its own test target called `<name>`, and
every `tests/<dir>/main.rs` as a target called `<dir>`. That target name is the only
reliable handle on an integration test, so it is what selection uses.

This was missing, and the cost was that the file scope could not name any of these
tests and exited with an error on a routine edit to one. They are 31 targets
holding 133 of the workspace's 4326 tests.

Two facts about this shape are easy to get wrong and both are handled here:

- **Their root-level tests all share the empty module path.** Twenty-one targets in
  one package report tests with no module segment at all, so a filter built from
  module names alone selects every one of them. Selection therefore always pairs
  the target name with the module names.
- **A directory with no `main.rs` builds no target.** It is shared code pulled in by
  other targets through `mod`, and it is named after no module, which is correct for
  that role rather than a convention violation.
"""

from __future__ import annotations

from pathlib import Path
from typing import NamedTuple

from owned_scanner import read_text, test_modules_in

ROOT = Path(__file__).resolve().parent.parent


class IntegrationSelection(NamedTuple):
    """The Cargo test target a `tests/` file builds into, and its modules."""

    binary: str | None
    entry: Path


def target_of(crate_dir: Path, rel: str) -> IntegrationSelection | None:
    """The test target holding `rel`, or None when the file is not under tests/.

    Returns None for a source file, which is the caller's signal to use the
    crate-root lookup instead.
    """
    target = Path(rel).resolve() if Path(rel).is_absolute() else (ROOT / rel).resolve()
    tests_dir = (crate_dir / "tests").resolve()
    if not target.is_relative_to(tests_dir):
        return None
    parts = target.relative_to(tests_dir).parts
    if not parts:
        return None
    if len(parts) == 1 and parts[0].endswith(".rs") and parts[0] != "mod.rs":
        return IntegrationSelection(parts[0][:-3], target)
    # tests/<dir>/main.rs is the target's own entry point.
    if len(parts) == 2 and parts[1] == "main.rs":
        return IntegrationSelection(parts[0], target)
    # tests/<dir>/mod.rs is shared code with no target of its own.
    if len(parts) == 2 and parts[1] == "mod.rs":
        return IntegrationSelection(None, target)
    # Any other file under tests/ belongs to the target owning its directory.
    directory = (tests_dir / parts[0])
    if directory.is_dir() and (directory / "main.rs").exists():
        return IntegrationSelection(parts[0], target)
    return None


def modules_in_target(entry: Path) -> list[str]:
    """Test modules inside one integration test target.

    A target's entry point is its own module root, so module paths are built the
    same way as for a crate: start at the entry file with an empty path and follow
    its declarations. Every file of the target is a test file, so every module in
    it that holds a test is a test module, with or without `cfg(test)`.
    """
    return test_modules_in(read_text(entry), "", entry, [], gated=True)


def declared_targets(crates: dict[str, Path]) -> dict[str, set[str]]:
    """Every integration target the given crates declare, with its modules.

    The crate map is passed in rather than imported. Reading cargo metadata is
    this repository's job, and importing it here would make the two modules
    import each other.
    """
    out: dict[str, set[str]] = {}
    for crate_dir in crates.values():
        for name, entry in target_entries(crate_dir).items():
            out.setdefault(name, set()).update(modules_in_target(entry))
    return out


def target_entries(crate_dir: Path) -> dict[str, Path]:
    """Each integration target of a crate, by name, with its entry file."""
    tests_dir = crate_dir / "tests"
    out: dict[str, Path] = {}
    if not tests_dir.is_dir():
        return out
    for entry in sorted(tests_dir.iterdir()):
        if entry.is_file() and entry.suffix == ".rs" and entry.name != "mod.rs":
            out[entry.stem] = entry
        elif entry.is_dir() and (entry / "main.rs").exists():
            out[entry.name] = entry / "main.rs"
    return out