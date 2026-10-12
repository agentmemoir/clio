"""Self-test cases for the scope lines the run prints before it runs anything."""

from __future__ import annotations

import io
import sys

from scoped_cli import main
from scoped_core import EXIT_OK

from scoped_selftest_support import shared as globals


def run(check) -> None:
    # The scope line names what the run is scoped to, not only what it judges.
    # Tests mode runs the packages that every changed Rust file selects, and that
    # set is wider than the judged one: a new test file is path-excluded from a
    # coverage report but still selects and runs its package's suite. Reading the
    # judged set here printed "0 changed file(s) to test" over that run (#82).
    original_select = globals()["select"]
    original_build_inputs = globals()["build_inputs"]
    original_changed_paths = globals()["changed_paths"]
    original_run_tests = globals()["run_tests"]
    try:
        # The selection is mocked, so the ambient tree's own diff must not reach
        # the run: a real build-input scan would add BUILD INPUT lines and turn
        # the file filter off. `run_tests` is a no-op so a mistaken path that
        # leaves plan mode cannot shell out to cargo; the checks below use
        # `--plan`, which returns before the run.
        globals()["build_inputs"] = lambda paths, packages: (set(), [])
        globals()["changed_paths"] = lambda base: []
        globals()["run_tests"] = lambda packages, filter_expression=None: 0

        def scope_lines(selection: tuple, argv: list[str]) -> tuple[int, str]:
            globals()["select"] = lambda base, everything: selection
            log = io.StringIO()
            original_stdout = sys.stdout
            try:
                sys.stdout = log
                code = main([*argv, "--plan", "--base", "master"])
            finally:
                sys.stdout = original_stdout
            return code, log.getvalue()

        code, text = scope_lines(
            (["clio"], [], ["crates/clio/tests/new_integration.rs"]),
            ["--tests-only"],
        )
        check(
            "a test-only change reports its files as tested, not zero",
            (
                code,
                "1 changed file(s) to test" in text,
                "test crates/clio/tests/new_integration.rs" in text,
                "1 package(s) to run: clio" in text,
            ),
            (EXIT_OK, True, True, True),
        )
        # Coverage mode names the judged subset, not the wider scoped set, because
        # its label says judge. Without this case the count could be widened in
        # both modes and still pass.
        code, text = scope_lines(
            (
                ["clio"],
                ["crates/clio/src/lib.rs"],
                ["crates/clio/src/lib.rs", "crates/clio/tests/new_integration.rs"],
            ),
            [],
        )
        check(
            "coverage mode reports the judged subset, not the scoped set",
            (
                code,
                "1 changed file(s) to judge" in text,
                "crates/clio/tests/new_integration.rs" in text,
            ),
            (EXIT_OK, True, False),
        )
    finally:
        globals()["select"] = original_select
        globals()["build_inputs"] = original_build_inputs
        globals()["changed_paths"] = original_changed_paths
        globals()["run_tests"] = original_run_tests
