"""Targeted test and coverage: run only the tests the changed files own.

This script IS the gate behind `make test-fast` and `make coverage-fast`.

Selection is the one `make quick` uses (`quick.select`, over
`scripts/owned_tests.py`): each changed Rust file selects the test modules it
declares, and a changed file under a crate's `tests/` directory selects its
integration target. Every selected test runs in ONE nextest run, over only the
packages that hold a selected test. No dependent package is added and
`--workspace` is never passed. When nothing is selected, no test runs.

Tests mode (`--tests-only`) runs that selection without instrumentation and
judges nothing: its exit code is nextest's.

Coverage mode runs the same selection once under `cargo llvm-cov nextest` and
judges every changed Rust file that is not exempt. Such a file fails when:

- it owns no test module of its own,
- the report does not hold it, or
- its line coverage is below `LINE_FLOOR` (80%).

Exempt, never judged: files under a crate's `tests/` directory, files that are
themselves declared test modules, `build.rs`, and files whose code is only
module declarations and re-exports. Function and region percentages are printed
and compared against nothing. 80% of lines is the only number, and no flag,
variable or file changes it.

A third mode, `--coverage-package NAME`, measures one package in its own
instrumented directory and judges nothing.

Usage:
  scripts/scoped_runner.py                         # coverage for changed files
  scripts/scoped_runner.py --tests-only            # the changed files' own tests
  scripts/scoped_runner.py --base master           # changed files vs a ref
  scripts/scoped_runner.py --plan                  # print the selection, run nothing
  scripts/scoped_runner.py --coverage-package NAME # one package, own dir, no verdict
  scripts/scoped_runner.py --self-test

Exit codes:
  0  every judged file reached 80% lines, or every selected test passed
  1  a judged file failed, or a selected test failed
  2  usage error
  3  the run or the report could not be produced (fail closed)
"""

from __future__ import annotations

import sys

from scoped_cli import main
from scoped_core import EXIT_OK, EXIT_RUN_ERROR, ROOT, RunError
from scoped_run import TEST_THREADS, hermetic_env
from scoped_select import (
    changed_files,
    changed_paths,
    embedders,
    git,
    is_present,
    merge_base,
    package_owners,
    resolve_ref,
    select,
    workspace_packages,
)

#: What `scripts/quick.py` and its helpers take from the scoped runner by name.
__all__ = [
    "EXIT_OK",
    "EXIT_RUN_ERROR",
    "ROOT",
    "TEST_THREADS",
    "RunError",
    "changed_files",
    "changed_paths",
    "embedders",
    "git",
    "hermetic_env",
    "is_present",
    "main",
    "merge_base",
    "package_owners",
    "resolve_ref",
    "select",
    "workspace_packages",
]

if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
