"""Running the tests and the coverage steps, and what a failed run prints."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path, PurePosixPath
from typing import Any, Collection, Iterable, NamedTuple, Sequence

from scoped_core import COV_DIR, REPORT, ROOT, RunError
from scoped_profiles import (
    clear_profile_buffers,
    drop_invalid_profraws,
    drop_selected_objects,
    instrumented_dir,
    prune_foreign_objects,
    resolved_instrumented_dir,
)


# --- running -------------------------------------------------------------------


def hermetic_env() -> dict[str, str]:
    """The environment every scoped run uses.

    CLIO_DEPLOYMENT_CONFIG is set for the same reason the Makefile sets it: a
    hand-run scoped command otherwise reads the developer's real deployment
    overlay, and a suite that asserts an empty one then fails on any machine that
    has a sidecar configured. The overlay path does not exist, which is the point.

    GATE_ACTIVE passes cargo through the gate wrapper unchanged, so a scoped run
    started from inside another gated command does not recurse into a cached
    verdict for itself.
    """
    env = dict(os.environ)
    env["CLIO_DEPLOYMENT_CONFIG"] = str(COV_DIR / "no-deployment-overlay.json")
    env["GATE_ACTIVE"] = "1"
    COV_DIR.mkdir(parents=True, exist_ok=True)
    stale = COV_DIR / "no-deployment-overlay.json"
    if stale.exists():
        stale.unlink()
    return env


TEST_THREADS = "2"
"""Test concurrency for both modes, and it is a flake mitigation, not a speed knob.

The suites share one Postgres database, and running many test binaries at once
against it has produced DDL contention here before. The old CI pass carried
`--test-threads 2` for exactly that reason, and dropping it silently let
concurrency rise to the runner's core count. Keep the cap: it costs wall clock
and buys a build that fails for real reasons only.
"""


def run_tests(
    packages: Sequence[str],
    filter_expression: str | None = None,
    targets: Sequence[str] = (),
) -> int:
    """Run tests without instrumentation, and return nextest's exit code.

    This is `make test-fast`. Under the crate scope it runs every test of every
    selected package; under the file scope it runs only the tests the changed
    files own, named by one filter expression.

    nextest's own exit code is returned unchanged, so a failing test fails the
    command exactly as it would fail `make test`. This tool adds no verdict of
    its own on the test side, because a test either passed or did not.

    A filter matching nothing exits non-zero, which is the behaviour relied on
    here: a selection that resolves to no test is an error rather than a quiet
    pass. Measured on this workspace's runner, and confirmed against the filter
    help, which documents the substring semantics.
    """
    env = hermetic_env()
    command = [
        "cargo", "nextest", "run",
        *[flag for name in packages for flag in ("-p", name)],
        "--locked", "--profile", "ci",
        "--test-threads", TEST_THREADS,
        *targets,
    ]
    if filter_expression:
        command += ["-E", filter_expression]
    print("running:", " ".join(command), flush=True)
    return subprocess.run(command, cwd=ROOT, env=env).returncode


def coverage_test_command(
    packages: Sequence[str],
    filter_expression: str | None = None,
    targets: Sequence[str] = (),
) -> list[str]:
    """Build and run the selected tests instrumented, and write no report.

    `--no-report` is what makes this one step rather than the whole run. Its
    documented meaning is "run tests, but don't generate coverage report", and
    it returns before the profile merge, so the buffers this step writes reach
    the disk untouched and the gate can act on them before anything reads them.

    Two flags that look right here and are not. `--no-run` means the opposite of
    what its name suggests: the tool documents it as "generate coverage report
    without running tests", and passing it rewrites the whole invocation into the
    report subcommand, so the tests this gate exists to run would not run at all.
    And `--no-clean` is rejected together with `--no-report`; the tool enables it
    implicitly, so passing both is a hard error before anything builds.
    """
    command = [
        "cargo", "llvm-cov", "nextest",
        *[flag for name in packages for flag in ("-p", name)],
        "--locked", "--profile", "ci",
        "--test-threads", TEST_THREADS,
        "--no-report",
        *targets,
    ]
    if filter_expression:
        command += ["-E", filter_expression]
    return command


def coverage_report_command(report_path: Path) -> list[str]:
    """Merge the profile buffers against the instrumented build and write JSON.

    No package selection here, deliberately. The report step's object list is
    chosen by walking the instrumented target directory for executables whose
    names match the workspace, and a package filter does not narrow that walk;
    restricting what the merge can see is this tool's job, and it does it by
    deleting the foreign objects before this command runs.

    `--failure-mode all` is the second line of defence behind dropping invalid
    buffers by size. Its documented meaning is "fail if `any` or `all` profiles
    cannot be merged", and `all` fails only when every buffer is unreadable, so
    one bad buffer loses its own coverage instead of the whole verdict.
    """
    return [
        "cargo", "llvm-cov", "report",
        "--json", "--output-path", str(report_path),
        "--failure-mode=all",
    ]


def run_coverage_tests(
    packages: Sequence[str],
    filter_expression: str | None = None,
    env: dict[str, str] | None = None,
    targets: Sequence[str] = (),
) -> None:
    """Build and run the selected tests instrumented, writing no report."""
    command = coverage_test_command(packages, filter_expression, targets)
    _run_or_raise(command, env or hermetic_env(), "the instrumented test run failed")


def write_coverage_report(
    report_path: Path,
    env: dict[str, str] | None = None,
) -> None:
    """Merge the profile buffers and write the JSON report at `report_path`."""
    command = coverage_report_command(report_path)
    _run_or_raise(command, env or hermetic_env(), "the coverage report step failed")


def run_coverage(
    packages: Sequence[str],
    filter_expression: str | None = None,
    report_path: Path | None = None,
    targets: Sequence[str] = (),
) -> None:
    """One instrumented pass, writing the scoped report path.

    Old profiles and selected executables are cleared first; otherwise the report can merge prior profiles or old line mappings with current binaries.

    The pass is two commands with the gate owning the gap between them, because
    the two things that have to happen in that gap cannot happen inside a single
    invocation: an unreadable profile buffer has to be dropped before the merge
    sees it, and an instrumented binary this run did not build has to be gone
    before the merge can pick it up.

    `filter_expression` narrows the run to the changed files' own tests, which is
    what makes the resulting number mean "this file's coverage of itself" rather
    than "this file's coverage by everything in the crate".
    """
    env = hermetic_env()
    clear_profile_buffers(env)
    drop_selected_objects(resolved_instrumented_dir(env), packages)
    run_coverage_tests(packages, filter_expression, env, targets)

    # The boundary the gate owns. Everything below acts on the instrumented build
    # the test step just produced and the report step is about to read.
    # Resolved against this run's environment, not the process's, so the hygiene
    # steps inspect the directory this run's commands actually built in.
    directory = instrumented_dir(env)
    drop_invalid_profraws(directory)
    prune_foreign_objects(directory, packages)

    write_coverage_report(report_path or REPORT, env)


TAIL_LINES = 20
"""How many lines of a captured run's own output to print on success."""


def _run_or_raise(command: Sequence[str], env: dict[str, str], headline: str) -> None:
    """Print the command, run it, and raise with its own output on failure.

    The captured output is reprinted before the error is raised. A run that ends
    with only an exit code has told the reader nothing they can act on: the whole
    diagnosis was in the tool's own message, which is what said a buffer's header
    was corrupt.

    The tail of the output is also printed on success, and that is the smaller of
    the two changes that make a long run legible. Streaming instead would show
    every line of a test run as it happens, which is the fuller answer, but it
    also loses the ability to reprint the streams cleanly on failure and changes
    what a successful run's log looks like in a way a reader of an existing log
    would have to learn again. A silent instrumented build for minutes reads as a
    hung build, and this repository has already spent an incident on exactly that
    misreading; the tail is enough to show the run finished and what it last
    said, at a cost of twenty lines.
    """
    print("running:", " ".join(command), flush=True)
    proc = subprocess.run(
        list(command), cwd=ROOT, env=env, capture_output=True, text=True
    )
    if proc.returncode == 0:
        _print_tail(proc.stdout, proc.stderr)
        return
    for stream, text in (("stdout", proc.stdout), ("stderr", proc.stderr)):
        if text and text.strip():
            print(f"scoped-runner: {headline}: {stream}:", file=sys.stderr)
            print(text.rstrip(), file=sys.stderr)
    raise RunError(f"{headline} (exit {proc.returncode})")


def _print_tail(stdout: str, stderr: str) -> None:
    """Print the last lines of a successful run's own output, so silence is never total."""
    for name, text in (("stdout", stdout), ("stderr", stderr)):
        lines = (text or "").splitlines()
        if not lines:
            continue
        tail = lines[-TAIL_LINES:]
        print(f"scoped-runner: the run finished; last {len(tail)} {name} line(s):")
        print("\n".join(tail))
