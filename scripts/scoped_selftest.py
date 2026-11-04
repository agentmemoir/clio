"""The scoped runner's self-test: the selection and judging rules, on inputs that cannot drift."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any, Collection, Iterable, NamedTuple, Sequence

from scoped_core import COV_DIR, EXIT_OK, owned

from scoped_selftest_select import run as run_select
from scoped_selftest_refs import run as run_refs
from scoped_selftest_steps import run as run_steps
from scoped_selftest_objects import run as run_objects
from scoped_selftest_modes import run as run_modes
from scoped_run import hermetic_env


def self_test() -> int:
    """The selection and judging rules, on inputs that cannot drift."""
    failures = 0

    def check(name: str, got: Any, want: Any) -> None:
        nonlocal failures
        if got != want:
            failures += 1
            print(f"self-test FAIL: {name}: got {got!r}, want {want!r}")
        else:
            print(f"self-test ok: {name}")

    printed, = run_select(check)
    raises, = run_refs(check)
    name, scratch, value = run_steps(check, raises)
    name, prune_packages, scratch, stale_map_ran = run_objects(check, raises)
    run_modes(check, name, prune_packages)

    # A scoped run owns its data directory. Inheriting one from HOME read the
    # developer's real erasure journal, which fails an unrelated test when that
    # journal was written under a different key salt. The path is compared
    # against the coverage directory, so the check does not depend on this host.
    # The foreign values are seeded first, so the check proves the override of an
    # inherited value and not only the default.
    foreign = {
        "CLIO_DATA_DIR": "/nonexistent/host-state/data",
        "CLIO_ERASURE_JOURNAL_FILE": "/nonexistent/host-state/erasure-journal.json",
    }
    saved = {name: os.environ.get(name) for name in foreign}
    try:
        os.environ.update(foreign)
        env = hermetic_env()
        data_dir = Path(env["CLIO_DATA_DIR"])
        check(
            "a scoped run reads a data directory it creates, under the coverage directory",
            (
                data_dir.is_absolute(),
                data_dir == COV_DIR / "data",
                data_dir.is_dir(),
                env["CLIO_DATA_DIR"] != foreign["CLIO_DATA_DIR"],
            ),
            (True, True, True, True),
        )
        check(
            "a scoped run drops an inherited erasure journal path",
            "CLIO_ERASURE_JOURNAL_FILE" in env,
            False,
        )
    finally:
        for name, value in saved.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value

# The lookup's own check, anchored to the test runner's list rather than to
    # this script's assumptions. A lookup verified only against itself passes
    # exactly when it is wrong, which is how the first two attempts at this failed
    # while reporting success. It needs the runner, so it is a separate switch
    # rather than part of the fast self-test.
    if os.environ.get("CLIO_ACCEPT_LOOKUP") == "1":
        print("self-test: lookup acceptance check against the test runner")
        if owned.accept_against_runner() != 0:
            failures += 1
            print("self-test FAIL: the lookup does not claim every test module")
        else:
            print("self-test ok: every test module the runner reports is claimed")

    summary = "all passed" if failures == 0 else f"{failures} failed"
    if failures == 0 and not stale_map_ran:
        # A skip is not a pass. The line a reader checks last has to carry it, or a
        # green summary hides a scenario that never ran on this host.
        summary += ", the stale source-map scenario was skipped"
    print(f"self-test: {summary}")
    return EXIT_OK if failures == 0 else 1
