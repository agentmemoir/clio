"""The scoped runner's self-test: the selection and judging rules, on inputs that cannot drift."""

from __future__ import annotations

import os
from typing import Any, Collection, Iterable, NamedTuple, Sequence

from scoped_core import EXIT_OK, owned

from scoped_selftest_select import run as run_select
from scoped_selftest_refs import run as run_refs
from scoped_selftest_steps import run as run_steps
from scoped_selftest_objects import run as run_objects
from scoped_selftest_modes import run as run_modes


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
    name, prune_packages, scratch = run_objects(check, raises)
    run_modes(check, name, prune_packages)

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

    print(f"self-test: {'all passed' if failures == 0 else f'{failures} failed'}")
    return EXIT_OK if failures == 0 else 1
