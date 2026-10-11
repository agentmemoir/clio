"""Targeted test and coverage: run only what a change touches.

This script IS the gate. CI runs it, and its verdict is the whole of what a
change has to clear: the tests of every crate the change can affect must pass,
and every Rust file the change created or modified must reach 80% line coverage.
There is no second, wider check behind it.

## The one coverage rule

> Every Rust file this change created or modified must reach 80% line coverage.

That is the whole of it: one metric, one number, one comparison. Nothing in this
file judges function coverage, region coverage, statement coverage, branch
coverage, an aggregate, a workspace total, a regression against a stored
baseline, the lines a diff added, or a per-file floor recorded anywhere. A
file's function percentage is measured and printed beside its line percentage
and compared against nothing, because printing a measurement is not a rule.

The number lives in one place, `LINE_FLOOR`, and no flag changes it. There is no
floor file to edit, no entry to add, and no way to raise or lower the bar for one
file or one run. A file below it fails; a file at or above it passes; there is no
third outcome and no per-file exemption.

Selection has two shapes, chosen by `--scope`. Both judge the same rule against
the same number, and differ only in which tests produce the measurement.

`--scope crate` (the default) works out which packages own the changed files,
adds every package that depends on them, runs that set, and judges the changed
files at `LINE_FLOOR`. Correct, and slow: a one-file change in a leaf crate runs
1636 of 4326 tests, because the root of the dependency graph owns 1382 of them
and every leaf crate's closure includes it.

`--scope file` runs, for each changed file, only the tests that file owns, and
judges that file's coverage from exactly those tests against the same
`LINE_FLOOR`. The test set comes from reading the file's own module
declarations, via `scripts/owned_tests.py`, never from a name guessed from a
filename. Guessing is what made the first attempt at this silently skip real
tests while reporting success; `scripts/owned_tests.py --accept` is the check
that would catch it, and it is part of the self-test.

The two selections the file scope runs are one filter expression, never two
invocations, so a test named by both runs once rather than twice.

It replaced a workspace-wide rule that this repository dropped: an aggregate 80%
floor over the whole workspace, and an 80% floor on every one of 507 reported
files, both produced by one instrumented pass over 184 crates and 4326 tests.
That pass was the reason a one-file change could not be verified here at all.
`make coverage` still produces a whole-workspace report on demand, but it judges
nothing, and nothing in CI runs it.

It answers one of two questions, and it selects the work the same way either
way:

  coverage mode  "are the files I just wrote at 80% lines?"
  tests mode     "do the crates my change can affect still pass?"

Both start from the same place. Work out which packages own the changed files,
then add every package that depends on them. A change to a leaf crate can move a
line count, or break a signature, in a crate that only calls it, so dependents
are included. The dependency graph comes from `cargo metadata`, so a file is
attributed the same way here as in the gate wrapper.

"Changed" means added, edited, or touched, against the merge base with `--base`.
It is not a narrower set than "created or modified": a new file is included, an
edited file is included, and so is a file whose only change was a comment. That
is deliberate. Judging the wider set is the stricter reading of the rule.

Coverage mode then runs one instrumented pass over those packages and judges
only the changed files. Tests mode runs the same packages without instrumentation
and returns nextest's own exit code, which is the fastest correctness check there
is: it skips the instrumented rebuild and the profile-data merge, and keeps every
test that could be affected by the change.

Why packages, and not test filters: a test filter decides which tests RUN, and
coverage is a side effect of what runs. Filtering tests to the changed files'
own test modules reports the coverage of those tests, which is not the coverage
of the changed files. Only scoping the run to the owning packages keeps the
measurement meaning the same thing it means in a whole-workspace report.

What this does NOT establish, stated plainly:

- Nothing here runs the tests of a crate the change cannot reach. A defect that
  needs an unrelated crate's suite to surface is no longer something this gate
  will catch. That is the price of the scoping, paid knowingly.
- A changed file can measure lower than a whole-workspace run would, because the
  tests that cover it may live in a package this run did not select. The package
  closure makes that rare, not impossible: a test in a package that merely *uses*
  a changed crate without appearing in its dependency edges would be missed. When
  a changed file reports an implausible drop, measure it wider before believing it.
- A branch that touches no Rust file is an honest all-clear, not a silent skip:
  it says so and exits 0. A `.md`-only change has nothing for either mode to run.
- A file with no instrumented line cannot be measured at all, so no coverage
  number applies to it. The run reads each changed file and names every one it
  skips for holding no executable code, rather than inventing a denominator of zero.

That divergence was measured rather than assumed: `compose_docker.rs` sits at
83.77% lines, close enough to the floor for a difference to show. Scoped to its
own package (1382 of 4326 tests) it measured 83.77, identical to the full run to
two decimal places. The package closure is what makes that hold, because the
tests that cover a changed crate live in the crates that depend on it, and the
closure includes every one of those.

Both modes fail closed. A base ref that does not resolve, or a changed Rust file
that belongs to no package and so can never appear in a report, is an error
rather than an empty scope, because an empty scope is indistinguishable from a
pass in the output.

A third mode measures one named package and judges nothing. It is what replaced
running the coverage tool's own per-package command by hand, because that command
produces badly wrong per-file rows: the tool builds a report's object list by
walking the whole instrumented target directory and keeps every executable whose
name matches the workspace, a package selection does not narrow that walk, and
the merge unions the objects it is given. Binaries an earlier run of other
packages left behind then move a file's line set and function count, which is
what produces a merge warning about functions with mismatched data and a
denominator that is a property of the build rather than of the file. This mode
gives each package its own instrumented directory, so the walk cannot see another
package's binary at all, and it still prunes and still drops unreadable buffers.
It reports; it does not gate. See the mode's own docstring for what that costs.

Usage:
  scripts/scoped_runner.py                        # coverage for changed files
  scripts/scoped_runner.py --tests-only            # tests for changed crates
  scripts/scoped_runner.py --base master          # changed files vs a ref
  scripts/scoped_runner.py --all                  # every package, changed files only
  scripts/scoped_runner.py --plan                 # print the selection, run nothing
  scripts/scoped_runner.py --scope file           # one file from its own tests
  scripts/scoped_runner.py --coverage-package NAME # one package, own dir, no verdict
  scripts/scoped_runner.py --self-test

Exit codes:
  0  every changed file reached 80% lines, or every selected test passed
  1  a changed file below 80% lines, a changed file the report does not contain,
     or a selected test failed
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
    git,
    is_present,
    merge_base,
    package_owners,
    resolve_ref,
    select,
    workspace_packages,
)

#: What `scripts/quick.py` takes from the scoped runner by name.
__all__ = [
    "EXIT_OK",
    "EXIT_RUN_ERROR",
    "ROOT",
    "TEST_THREADS",
    "RunError",
    "changed_files",
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
