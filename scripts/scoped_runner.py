#!/usr/bin/env python3
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

import argparse
import hashlib
import io
import json
import os
import re
import stat
import subprocess
import sys
import tempfile
from pathlib import Path, PurePosixPath
from typing import Any, Collection, Iterable, NamedTuple, Sequence

ROOT = Path(__file__).resolve().parent.parent

sys.path.insert(0, str(Path(__file__).resolve().parent))
try:
    import owned_tests as owned
except ImportError as error:  # pragma: no cover - only when the file is missing
    print(f"scoped-runner: cannot import scripts/owned_tests.py: {error}", file=sys.stderr)
    raise

try:
    from executable_code import offenders
except ImportError as error:  # pragma: no cover - only when the file is missing
    print(
        f"scoped-runner: cannot import scripts/executable_code.py: {error}",
        file=sys.stderr,
    )
    raise

OwnedLookupError = owned.ResolutionError

LINE_FLOOR = 80.0
"""The one coverage number in this repository.

A single fixed percentage, applied to every Rust file a change created or
modified, with no per-file override and no flag that changes it. It was a
per-file recorded floor once, and a per-file recorded floor is two rules: the
fixed number for most files and a second, lower number for whoever's file was
recorded. Recording one also stored the denominator it was measured over, which
made a file's floor depend on which report produced it, so the same tree could
pass on one machine and fail on another.

The metric is line coverage and only line coverage. Function and region
percentages are measured and printed, and neither is compared against this
number or anything else. There is no aggregate, no regression baseline, and no
second gate on the lines a diff added.
"""
COV_DIR = ROOT / "target" / "coverage"
# Its own report path, deliberately NOT the full gate's `coverage.json`.
# Sharing one path meant a fast run silently overwrote the artifact the whole
# report produced, so a later read of `coverage.json` saw a partial tree and
# reported it as if it were the workspace.
REPORT = COV_DIR / "coverage-scoped.json"

EXIT_OK = 0
EXIT_FAIL = 1
EXIT_USAGE = 2
EXIT_RUN_ERROR = 3

# cargo-llvm-cov builds an ignore regex and hands it to llvm-cov, so the paths
# below never reach a report and must not be treated as "changed but unreported".
#
# The directory half is the tool's rule, read from its source rather than its
# README: it anchors `tests|examples|benches` to the workspace root but lets the
# span between match any depth, so `crates/x/tests/a.rs` and a `tests/` folder
# nested inside `src/` are both hidden. The check in `is_measureable` is a plain
# path-component test rather than that anchored regex: it accepts the same
# paths, and it also holds under the alternate regex the tool uses when path
# remapping is on. The looser form is ours. The rule it mirrors is the tool's.
#
# The suffix half is the tool's rule too, and narrower than a suffix list looks:
# it matches `tests.rs` and `<word characters>[_-]tests.rs`, never a bare
# `_tests.rs`. Keeping a suffix list here is harmless, because the wider match is
# a file the tool also hides.
EXCLUDED_DIRS = ("tests", "examples", "benches")
EXCLUDED_SUFFIXES = ("_tests.rs", "-tests.rs", "tests.rs")

RAW_PROFILE_HEADER_BYTES = 128
"""The size of the header every raw profile buffer starts with.

Not a guess and not a minimum file size that happens to work: it was measured on
this workspace's pinned toolchain by truncating a real buffer. A 127-byte prefix
is rejected by the merge tool as `invalid instrumentation profile data (file
header is corrupt)`, and a 128-byte one parses and reports zero functions, which
is exactly what an allocated-but-never-written merge-pool slot looks like.

The gate deletes every buffer shorter than this before the merge, because one of
them otherwise fails the whole merge after every test has already passed. The
cost of that is one test's coverage missing from the report, so the number
dropped is printed on every run rather than only when it is not zero.
"""

# The directory name the coverage tool appends to the workspace target directory
# for its instrumented build. Read from the tool's own source rather than
# guessed: with no explicit target directory configured, it builds into
# `<target>/llvm-cov-target`, because changing the compiler flags would otherwise
# invalidate the ordinary build cache.
#
# `cargo llvm-cov show-env` deliberately reports the ORDINARY target directory
# rather than this one, so it cannot be used to find the instrumented build.
INSTRUMENTED_DIR_NAME = "llvm-cov-target"

# Where a one-package measurement builds, relative to the workspace root.
#
# A directory of its own per package, because the fix for the merge this mode
# exists for is not a cleverer object rule: it is a walk that cannot see
# anything it should not. The tool resolves its target directory in three
# branches, read from its source rather than guessed. An explicit target
# directory wins; otherwise the diagnostic subcommand reports the ordinary one;
# only a real run falls back to the instrumented subdirectory above. So the
# explicit override is the only value that is both effective and stated, and a
# per-package path under it is what keeps one package's instrumented binaries out
# of the next package's merge.
PACKAGE_COV_DIR_NAME = "llvm-cov-package"

# The two environment variables that place a coverage build. There is no flag for
# either, which is why a caller pins the directory through the environment and
# then has to resolve it by the same rule the tool uses.
CARGO_LLVM_COV_TARGET_KEY = "CARGO_LLVM_COV_TARGET_DIR"
CARGO_LLVM_COV_BUILD_KEY = "CARGO_LLVM_COV_BUILD_DIR"

# Extensions the tool never treats as a coverage object. Without this, a
# `.rlib` or `.rmeta` whose stem matches a package name would look exactly like a
# binary to a name-based rule, and deleting one would force a rebuild.
NOT_OBJECT_SUFFIXES = (".d", ".rlib", ".rmeta", ".cargo-lock", ".cargo-build-lock")

# Directories the tool skips when it collects objects. Copied rather than
# re-derived so this rule and the tool's cannot drift apart silently.
SKIPPED_BUILD_DIRS = ("incremental", ".fingerprint", "fingerprint")


class RunError(Exception):
    """The measurement could not be produced, so no verdict can be given."""


class CoverageRow(NamedTuple):
    """One file's row: its line percentage, then its function percentage.

    Both percentages are read because the report carries both and a report that
    dropped one of them would be a different measurement. Only the line half is
    judged, by `judge`, and the function half is printed beside it wherever a
    line percentage is printed. Nothing compares a function percentage against
    anything.

    The counts are read for the same reason and judged by nothing. They were what
    a per-file recorded floor was compared against, and that rule is gone.
    """

    lines_pct: float
    functions_pct: float
    lines_count: int | None = None
    functions_count: int | None = None
    lines_covered: int | None = None
    functions_covered: int | None = None


# --- selecting what to measure -------------------------------------------------


def git(*args: str) -> str:
    """Run one git query in this repository and return its standard output.

    `core.quotePath=false` is passed on every call, because a path holding a
    space, a backslash or a non-ASCII byte is otherwise written back quoted with
    C-style escapes, and a caller comparing that against a repository path never
    matches. The two shapes for the same query then disagree, which is how a
    quoted path charges one file's lines to another. The alternative used for the
    untracked-file query is a NUL separator, which is stronger, but it cannot be
    used for the queries whose output this module reads line by line.
    """
    proc = subprocess.run(
        ["git", "-c", "core.quotePath=false", *args],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        raise RunError(f"git {' '.join(args)} failed: {proc.stderr.strip()}")
    return proc.stdout


def _ref_exists(ref: str) -> bool:
    """True when `ref` names something git can resolve.

    A branch with no merge base is a real state (a first commit, or a base that
    has moved out of reach), and it is handled by falling back to the working
    tree. A ref that does not exist at all is a typo, and a typo must not become
    an empty scope.
    """
    proc = subprocess.run(
        ["git", "rev-parse", "--verify", "--quiet", f"{ref}^{{commit}}"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    return proc.returncode == 0


def resolve_ref(base: str) -> str | None:
    """The spelling of `base` that git can actually resolve here, or None.

    A bare branch name does not always resolve, and the case that matters is the
    one CI hits. `actions/checkout` checks a pull request out in detached HEAD
    mode: there is no local branch for the base at all, only
    `refs/remotes/origin/<base>`. So on a runner `--base master` is an
    unresolvable ref while `--base origin/master` is a perfectly good one, and
    the same command that works on a developer's machine fails on the runner.

    Trying the remote-tracking form second is what makes one command correct in
    both places. It is a fallback rather than a replacement so a developer who
    has a local `master` still gets the local one, which is what they mean.

    Returns None when neither form resolves, and the caller treats that as an
    error rather than as an empty scope.
    """
    for candidate in (base, f"origin/{base}"):
        if _ref_exists(candidate):
            return candidate
    return None


def merge_base(base: str) -> str | None:
    """The commit this branch diverged from, or None when there is not one yet.

    A branch with no upstream, or a first commit, has no merge base. That is not
    an error to hide: the caller falls back to measuring everything, because
    guessing a smaller scope would under-report.
    """
    ref = resolve_ref(base)
    if ref is None:
        return None
    proc = subprocess.run(
        ["git", "merge-base", "HEAD", ref],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        return None
    sha = proc.stdout.strip()
    return sha or None


def changed_files(base: str) -> list[str]:
    """Repository-relative paths of changed Rust source files.

    Four git queries, each owning exactly one case, because a single status
    parse cannot separate them reliably: committed work since the merge base,
    staged work, unstaged work, and untracked files. A new source file is the
    case most likely to be below the floor, because it arrives with no tests, so
    leaving untracked files out would defeat the purpose of the tool.

    Untracked files come from `git ls-files --others`, not from `git status`, and
    that is not a style preference. `git status --porcelain` collapses a wholly
    untracked directory to one entry ending in a slash, so every Rust file below
    it was invisible: the `.rs` filter dropped the directory entry and the gate
    read the tree as clean. That is the worst direction for this tool, because a
    new source file is the case most likely to be below the floor. `ls-files`
    lists files, not directories, so a new crate or test-support folder arrives
    one path per file.
    """
    paths: set[str] = set()
    sha = merge_base(base)
    if sha is not None:
        paths.update(
            line for line in git("diff", "--name-only", f"{sha}..HEAD").splitlines() if line
        )
    paths.update(
        line for line in git("diff", "--name-only", "--cached").splitlines() if line
    )
    paths.update(line for line in git("diff", "--name-only").splitlines() if line)
    # `-z` so a path containing a space or a quote arrives whole rather than
    # quoted, and `--exclude-standard` so the same ignore rules apply here as in
    # every other status query above.
    for entry in git("ls-files", "--others", "--exclude-standard", "-z").split("\0"):
        if entry:
            paths.add(entry)
    return sorted(p for p in paths if p.endswith(".rs"))


def is_measureable(rel: str) -> bool:
    """True when llvm-cov would put this file in a report at all.

    Both halves of the tool's exclusion have to be here, not just the filename
    half. Judging a file the report cannot contain is a false failure: the run
    reports every measured file above its floor and still exits non-zero, which
    is what blocked every change that added an integration test.
    """
    if rel.endswith(EXCLUDED_SUFFIXES):
        return False
    return not set(PurePosixPath(rel).parts[:-1]) & set(EXCLUDED_DIRS)


def has_no_executable_code(rel: str) -> bool:
    """True when the source at `rel` compiles to no counted line.

    The decision is read from the file: a file holds no counted line exactly when
    its source has no function body and no macro. A file that cannot be read is
    not codeless, so the safe direction is to judge it; `is_present` handles a
    deleted file separately.
    """
    try:
        source = (ROOT / rel).read_text(encoding="utf-8")
    except (OSError, UnicodeError):
        return False
    return not offenders(source)


def parse_codeless(files: Iterable[str]) -> set[str]:
    """The measured files whose own source holds no executable code."""
    return {rel for rel in files if is_measureable(rel) and has_no_executable_code(rel)}


def is_judgeable(rel: str, codeless: Collection[str] = ()) -> bool:
    """True when this file is one a report could hold and must be judged against.

    This is the judging half only. Selection still reads every changed Rust file,
    so a crate root that holds nothing but declarations still selects its own
    package and its tests still run: the file is skipped by the report, not by
    the test run.
    """
    return is_measureable(rel) and rel not in codeless


def is_present(rel: str) -> bool:
    """True when the path still exists in the working tree.

    A deleted file is a changed file with nothing left to measure, not a
    resolution failure. The ownership lookup walks the crate roots that exist
    now, so a path that has been removed cannot be reached from any of them and
    the lookup raises. Failing the run over a file the change removed would make
    the gate unrunnable for exactly the edits that consolidate test ownership,
    which is the work the convention asks people to do.
    """
    return (ROOT / rel).exists()


def package_owners(rel: str, packages: Sequence[dict[str, Any]] | None = None) -> list[str]:
    """Package names whose directory contains `rel`.

    The mapping is directory-to-name taken from `cargo metadata`, NOT the
    directory name itself. The two are usually the same and sometimes are not;
    where they differ, handing the directory name to `cargo llvm-cov -p` gives a
    value that does not resolve and fails the run. The longest matching directory
    prefix wins, so a nested path attributes to the crate that actually contains
    it.
    """
    if packages is None:
        packages = workspace_packages()
    best: tuple[int, str] | None = None
    for package in packages:
        root = package.get("dir")
        if not root:
            continue
        prefix = root.rstrip("/") + "/"
        if rel.startswith(prefix) and (best is None or len(prefix) > best[0]):
            best = (len(prefix), package["name"])
    return [best[1]] if best else []


def workspace_packages() -> list[dict[str, Any]]:
    """Every workspace member and its path dependencies, from cargo metadata.

    `--no-deps` keeps the graph to workspace members, so the closure below is
    over local packages only and cannot wander into the registry.
    """
    env = dict(os.environ, GATE_ACTIVE="1")
    proc = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--locked", "--format-version", "1"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        env=env,
    )
    if proc.returncode != 0:
        raise RunError(f"cargo metadata failed: {proc.stderr.strip()}")
    try:
        packages = json.loads(proc.stdout)["packages"]
    except (ValueError, KeyError, TypeError) as error:
        raise RunError(f"cargo metadata is not usable: {error}") from error
    by_dir: dict[str, str] = {}
    for package in packages:
        by_dir[str(Path(package["manifest_path"]).parent)] = package["name"]
    out = []
    for package in packages:
        deps = set()
        for dep in package.get("dependencies", []):
            path = dep.get("path")
            if path and str(Path(path)) in by_dir:
                deps.add(by_dir[str(Path(path))])
        manifest_dir = Path(package["manifest_path"]).parent
        try:
            # Git reports repository-relative paths and cargo reports absolute
            # ones. Everything downstream compares against git, so the mapping
            # is stored in git's form and the two are made comparable once here.
            directory = str(manifest_dir.resolve().relative_to(ROOT))
        except ValueError:
            directory = str(manifest_dir)
        out.append(
            {
                "name": package["name"],
                "deps": deps,
                # Directory-to-package mapping, because they are not always equal.
                "dir": directory,
                # Every target name, because a package's executables are named
                # after its TARGETS and not only after the package: an integration
                # test target called `mcp_principal_binding` produces an executable
                # that no package name matches.
                "targets": [target["name"] for target in package.get("targets", [])],
            }
        )
    return out


def package_closure(seeds: Iterable[str], packages: Sequence[dict[str, Any]]) -> list[str]:
    """`seeds` plus every package that transitively depends on one of them.

    Dependents are included because a change to a crate they call can change how
    many of their own lines run. Measuring only the edited crate would report the
    edited crate's coverage and say nothing about the caller that a reviewer will
    actually look at next.
    """
    dependents: dict[str, set[str]] = {}
    for package in packages:
        for dep in package["deps"]:
            dependents.setdefault(dep, set()).add(package["name"])
    out = set(seeds)
    stack = list(seeds)
    while stack:
        for name in dependents.get(stack.pop(), ()):
            if name not in out:
                out.add(name)
                stack.append(name)
    return sorted(out)


def select(base: str, everything: bool) -> tuple[list[str], list[str], list[str]]:
    """The packages to run, the changed files to judge, and every changed file.

    The two file lists differ on purpose. `judgeable` is what a coverage report
    could ever contain; `changed` is everything the commit touched, including
    the files llvm-cov path-excludes. Selection reads `changed`, because a test
    you just wrote has to run even though it can never be measured.

    Fails closed. An unresolvable base, or a dirty tree that yields no Rust file
    to judge, raises rather than returning an empty scope, because an empty
    scope is indistinguishable from a pass in the output and would report a
    clean run over work that was never measured.
    """
    if resolve_ref(base) is None:
        raise RunError(
            f"base ref {base!r} (nor origin/{base}) resolves to no commit, so the "
            f"changed files cannot be determined; refusing to report a verdict over "
            f"nothing. In a checkout with no local branch for the base, which is "
            f"what CI uses, pass the remote form."
        )
    files = changed_files(base)
    # A file the change deleted joins the other two exclusions, because it is the
    # same case: there is nothing left in it for a report to hold. Left in the
    # judged list, it was reported as a file the run failed to measure, so every
    # scope failed a change that only removed a file.
    codeless = parse_codeless(files)
    judgeable = [f for f in files if is_judgeable(f, codeless) and is_present(f)]
    packages = workspace_packages()
    if everything:
        return [p["name"] for p in packages], judgeable, files
    if not files:
        # A genuinely clean tree: no changed file of any type.
        return [], judgeable, files
    # Every file a report could hold, codeless ones included. The check is about
    # belonging to a package at all, which has nothing to do with whether the file
    # has code, so it must not lose the codeless files the judging set just dropped.
    unowned = [f for f in files if is_measureable(f) and not package_owners(f, packages)]
    if unowned:
        # A changed Rust file no package owns can never appear in a report, so it
        # cannot be judged. Refusing here is the whole point of the check: the
        # alternative is a clean-looking run that never measured the file.
        raise RunError(
            "changed Rust file(s) belong to no workspace package and can never "
            f"appear in a coverage report: {', '.join(unowned)}; refusing to "
            "report a verdict over files that cannot be measured"
        )
    owners: set[str] = set()
    for rel in files:
        # Every changed Rust file seeds the selection, including the ones llvm-cov
        # path-excludes. Judging still uses `judgeable`, because an excluded file
        # can never appear in a report, but SELECTION cannot ignore it: a commit
        # that only adds or edits a test file changes nothing in `judgeable`, and
        # seeding from `judgeable` alone left the closure empty, so the gate
        # selected no package and ran no tests at all for exactly the commits that
        # added a test. A test you just wrote has to be run.
        owners.update(package_owners(rel, packages))
    if not owners:
        # Only non-Rust files changed: docs, a Makefile, or a Python script.
        # Nothing there can move a Rust line count.
        return [], judgeable, files
    return package_closure(owners, packages), judgeable, files


# --- judging the report --------------------------------------------------------


def load_report(path: Path) -> dict[str, Any]:
    try:
        doc = json.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        raise RunError(f"cannot read report {path}: {error}") from error
    except ValueError as error:
        raise RunError(f"report is not valid JSON: {error}") from error
    if not isinstance(doc, dict) or not isinstance(doc.get("data"), list):
        raise RunError(f"{path} is not an llvm-cov JSON report")
    return doc


def report_rows(doc: dict[str, Any]) -> dict[str, CoverageRow]:
    """Per-file percentages and counts, keyed by a repo-relative path.

    The report spells paths absolutely and differently on every machine, so the
    key is the path relative to this checkout. That is what makes a row
    comparable to a changed-file list produced by git.

    A row whose percentages are missing is dropped, because there is nothing to
    judge. A row whose COUNTS are missing is kept, with the count left unknown,
    because a missing denominator is a shape this tool cannot judge and must say
    so about rather than invent.
    """
    rows: dict[str, CoverageRow] = {}
    for block in doc["data"]:
        for entry in block.get("files", []):
            filename = entry.get("filename")
            summary = entry.get("summary")
            if not isinstance(filename, str) or not isinstance(summary, dict):
                continue
            lines = summary.get("lines", {})
            functions = summary.get("functions", {})
            if not isinstance(lines.get("percent"), (int, float)):
                continue
            if not isinstance(functions.get("percent"), (int, float)):
                continue
            try:
                key = str(Path(filename).resolve().relative_to(ROOT))
            except ValueError:
                key = filename
            rows[key] = CoverageRow(
                float(lines["percent"]),
                float(functions["percent"]),
                _count(lines),
                _count(functions),
                _covered(lines),
                _covered(functions),
            )
    return rows


def _count(summary: dict[str, Any]) -> int | None:
    """The denominator in a summary block, or None when the report omits it.

    `None` and `0` are different answers and only one of them is allowed to be
    stored. A missing count means the report no longer says how much it
    measured, which the caller has to treat as a refusal; a zero means it
    measured nothing, which is a measurement.
    """
    value = summary.get("count")
    return value if isinstance(value, int) and not isinstance(value, bool) else None


def _covered(summary: dict[str, Any]) -> int | None:
    """The covered numerator in a summary block, or None when it is absent.

    This is the number a recorded count is compared against, so it has to come
    from the report rather than be recomputed from the percentage: the percentage
    is already rounded, and a numerator recovered from a rounded percentage is
    not the count the run measured.
    """
    value = summary.get("covered")
    return value if isinstance(value, int) and not isinstance(value, bool) else None


def renames(base: str) -> set[str]:
    """Named paths this change only moved, and did not edit.

    A pure rename produces no hunk, so the file's added-line set is empty and it
    would be reported the same way as one this change never touched. The two are
    different facts: nothing was added, because nothing about the content
    changed, and a reader looking at a coverage verdict for a file they only
    moved deserves to be told which of the two happened.

    This is a diagnostic, not a rule. Nothing about a rename changes a verdict:
    the file is judged on its own line coverage either way. It was removed by
    accident when the added-line gate it used to share a data path with went, and
    it is back because the information is about the diff rather than about
    coverage.
    """
    sha = merge_base(base)
    if sha is None:
        return set()
    moved: set[str] = set()
    # `-M` because rename detection is not implied by every diff call, and the
    # filter so only renames reach this parse.
    records = git(
        "diff", "--name-status", "-M", "--diff-filter=R", "-z", sha
    ).split("\0")
    for index in range(0, len(records) - 2, 3):
        status, _old, new = records[index : index + 3]
        if status.startswith("R"):
            moved.add(new)
    return moved


def file_scope_filter(files: Sequence[str]) -> tuple[str, list[str]]:
    """One filter expression naming every test `files` own, and the files owning none.

    Only valid for the test verdict, where a test either passed or did not. It is
    NOT valid for coverage: one run over several files' tests produces one merged
    report, and each file's row is then the union of all of their tests' coverage.
    Measured: `create.rs` reads 81.82% lines from its own tests and 92.12% when a
    second changed file's tests run alongside. `judge_file_scope` therefore runs
    one pass per file and this union is never used to measure.
    """
    selections = [owned.owned_selection(rel) for rel in files]
    unplanned = [s.file for s in selections if s.is_empty]
    modules: set[str] = set()
    binaries: set[str] = set()
    for selection in selections:
        modules.update(selection.modules)
        if selection.binary:
            binaries.add(selection.binary)
    if binaries:
        # Mixed targets: one branch per target, each naming only that target's
        # modules. A single `binary()` would name at most one target, and a
        # name-only union would pull in every target sharing a module path.
        branches = []
        for binary in sorted(binaries):
            own = {m for s in selections if s.binary == binary for m in s.modules}
            branches.append(owned.filter_expression(own, binary))
        for selection in selections:
            if selection.binary is None:
                branches.append(owned.filter_expression(selection.modules))
        return "(" + " | ".join(b for b in branches if b) + ")", unplanned
    return owned.filter_expression(modules), unplanned


def report_verdicts(
    offenders: Sequence[str],
    unreported: Sequence[str],
    stream: Any,
) -> None:
    """Name each file the report did not hold, then each file below the floor.

    One helper for both scopes because a count is not a diagnosis: a reader
    holding only "22 below floor" has to diff the printed pass list against the
    judged list to learn which files failed. The stream is a parameter because
    the two scopes already disagree about where these lines belong, and moving
    them would change output some reader depends on.
    """
    for rel in unreported:
        print(f"scoped-runner: NOT MEASURED {rel}", file=stream)
    for offender in offenders:
        print(f"scoped-runner: BELOW FLOOR {offender}", file=stream)


def judge_file_scope(
    packages: Sequence[str],
    files: Sequence[str],
    floor: float,
) -> tuple[list[str], list[str]]:
    """Measure and judge each changed file from that file's own tests, one run each.

    One run per file, not one run for the lot. A single instrumented pass over
    several files' tests writes one merged report, and in that report every
    file's row is the coverage contributed by all of them. That is not a small
    bias: measured here, `create.rs` reads 89.70% lines from its own tests and
    92.12% once a second changed file's tests join the run. Under that
    arrangement a file can be passed by another file's tests, which is the one
    thing a per-file measurement is for.

    The cost is one instrumented run per changed file rather than one per change.
    That is the right trade because a change usually touches a handful of files,
    and because correctness here is not recoverable from a faster wrong number.

    The judgement itself is `judge`, the same function the crate scope uses, at
    the same number. This scope differs in which tests produced the measurement
    and in nothing else.
    """
    offenders: list[str] = []
    unmeasured: list[str] = []
    for rel in files:
        selection = owned.owned_selection(rel)
        if selection.is_empty:
            # No test of its own means 0% coverage, which is a verdict rather than
            # a gap: the touch rule requires the file to be given tests.
            offenders.append(
                f"{rel} lines 0.00% (floor {floor:.2f}, no test module found)"
            )
            continue
        owners = package_owners(rel)
        expression = selection.expression()
        try:
            run_coverage(owners or packages, expression, report_path_for(rel))
            doc = load_report(report_path_for(rel))
        except RunError as error:
            raise RunError(f"{rel}: {error}") from error
        rows = report_rows(doc)
        below, missing = judge(rows, [rel], floor)
        offenders.extend(below)
        unmeasured.extend(missing)
        if not below and not missing:
            row = rows[rel]
            # Both metrics are printed, and only the lines one was judged: a report
            # that hid half of what it measured would be a different measurement.
            print(
                f"scoped-runner: {rel} lines {row.lines_pct:.2f}% "
                f"functions {row.functions_pct:.2f}% "
                f"from its own test module(s); judged on lines"
            )
    return offenders, unmeasured


def report_path_for(rel: str) -> Path:
    """A per-file report path, so one file's measurement cannot overwrite another's.

    A single shared path would let the last run's report stand in for the others,
    which is how a targeted number silently becomes a whole-crate one.
    """
    digest = hashlib.sha256(rel.encode("utf-8")).hexdigest()[:12]
    return COV_DIR / f"coverage-file-{digest}.json"


def judge(
    rows: dict[str, CoverageRow],
    files: Sequence[str],
    floor: float = LINE_FLOOR,
) -> tuple[list[str], list[str]]:
    """The coverage rule, in one place. Returns (offenders, unreported).

    Two lists, because two different things can be true of a changed file, and
    collapsing them would hide one:

    - `offenders`: the file is in the report and its line coverage is below
      `floor`. That is a failure, and it is the only failure this rule has.
    - `unreported`: the file is absent from the report. That is a gap and neither
      a pass nor a failure. Reporting it as green would be the exact lie this
      tool exists to avoid, so it is named and the run exits non-zero.

    The comparison is the line percentage against the one number in this
    repository, and nothing else is compared against anything. A file's function
    percentage is printed beside its line percentage and judged by no code path,
    because a percentage nobody compares is a measurement rather than a rule.

    There is deliberately no per-file override here and no argument to add one.
    A file that cannot reach the floor is a file whose tests do not exist yet,
    and the only honest way to record that is to write the tests, not to store
    its current number in a file the gate then reads as permission.
    """
    offenders: list[str] = []
    unreported: list[str] = []
    for rel in files:
        row = rows.get(rel)
        if row is None:
            unreported.append(rel)
            continue
        if row.lines_pct < floor:
            offenders.append(
                f"{rel} lines {row.lines_pct:.2f}% (floor {floor:.2f})"
            )
    return offenders, unreported


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
    ]
    if filter_expression:
        command += ["-E", filter_expression]
    print("running:", " ".join(command), flush=True)
    return subprocess.run(command, cwd=ROOT, env=env).returncode


def coverage_test_command(
    packages: Sequence[str],
    filter_expression: str | None = None,
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
) -> None:
    """Build and run the selected tests instrumented, writing no report."""
    command = coverage_test_command(packages, filter_expression)
    _run_or_raise(command, env or hermetic_env(), "the instrumented test run failed")


def write_coverage_report(
    report_path: Path,
    env: dict[str, str] | None = None,
) -> None:
    """Merge the profile buffers and write the JSON report at `report_path`."""
    command = coverage_report_command(report_path)
    _run_or_raise(command, env or hermetic_env(), "the coverage report step failed")


def clear_profile_buffers(env: dict[str, str]) -> None:
    """Delete this run's predecessor profile buffers, keeping the warm build.

    `--profraw-only` is the point: it removes the profile files and leaves the
    instrumented build alone. A plain `--workspace` clean throws the whole
    directory away, which is the opposite of what an incremental run wants.

    A non-zero exit is a refusal rather than a warning, and the output is printed
    rather than kept. The clean is what stops this run's report merging the last
    run's buffers, so a clean that quietly failed leaves exactly the stale
    number this gate exists to remove, with nothing in the log to say so. The
    captured streams are discarded once they have been reprinted.
    """
    proc = subprocess.run(
        ["cargo", "llvm-cov", "clean", "--profraw-only"],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        for stream, text in (("stdout", proc.stdout), ("stderr", proc.stderr)):
            if text and text.strip():
                print(f"scoped-runner: the profile-buffer clean: {stream}:")
                print(text.rstrip())
        raise RunError(
            "the profile-buffer clean failed (exit "
            f"{proc.returncode}), so this run's report would merge the previous "
            "run's profile buffers and report coverage for code that did not run. "
            "Refusing to publish a report measured that way."
        )


def run_coverage(
    packages: Sequence[str],
    filter_expression: str | None = None,
    report_path: Path | None = None,
) -> None:
    """One instrumented pass, writing the scoped report path.

    Old profile data is cleared first. Without that, the report merges this run
    with the last one and reports coverage for code that did not run, which is
    the most likely way a targeted number silently becomes a full-workspace one.
    This is not theoretical: a stale-profile leak was checked for directly while
    building the file scope, and the clean step does drop every `.profraw`.

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
    run_coverage_tests(packages, filter_expression, env)

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


def resolved_instrumented_dir(env: dict[str, str] | None = None) -> Path:
    """Where the instrumented build is, whether or not it exists yet.

    Split from the check below so the resolution itself is testable without a
    build on disk. An override wins; otherwise the instrumented subdirectory of
    the workspace target directory is used, and that subdirectory is the reason
    the ordinary `target/` is never touched.

    `env` is the environment the run is building from, passed in rather than read
    from the process, because a caller that pins the directory for one run has to
    be sure this function resolves the same one the tool will. The tool's
    diagnostic subcommand cannot answer the question: it reports the ordinary
    target directory by design, so the value it prints is not where a real run
    puts anything.
    """
    source = os.environ if env is None else env
    override = source.get(CARGO_LLVM_COV_TARGET_KEY)
    if override:
        return (ROOT / override).resolve()
    proc = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--locked", "--format-version", "1"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        env=dict(os.environ, GATE_ACTIVE="1"),
    )
    if proc.returncode != 0:
        raise RunError(
            f"cargo metadata failed, so the instrumented build directory "
            f"cannot be resolved: {proc.stderr.strip()}"
        )
    try:
        target_directory = json.loads(proc.stdout)["target_directory"]
    except (ValueError, KeyError, TypeError) as error:
        raise RunError(
            f"cargo metadata is not usable, so the instrumented build "
            f"directory cannot be resolved: {error}"
        ) from error
    return Path(target_directory).resolve() / INSTRUMENTED_DIR_NAME


def instrumented_dir(env: dict[str, str] | None = None) -> Path:
    """The instrumented build's own directory, or an error naming what failed.

    The instrumented directory is where the profile buffers and the instrumented
    executables both live, so both of the steps below need it and neither can
    infer it from finding nothing: a wrong directory reports zero buffers and
    zero foreign objects, which reads exactly like a clean run. So absence is an
    error here rather than a measurement of nothing.

    Resolution is `resolved_instrumented_dir`, so a caller that pins the
    directory for one run resolves the same one the tool will build in.
    """
    base = resolved_instrumented_dir(env)
    if not base.is_dir():
        raise RunError(
            f"the instrumented build directory {base} does not exist, so the "
            f"profile buffers and the instrumented executables this run must "
            f"inspect cannot be found. Refusing to report a verdict over a build "
            f"that was never measured. An override naming a directory that does "
            f"not exist is the usual cause."
        )
    return base


def _inside(child: Path, parent: Path) -> bool:
    """True when `child` really is under `parent`, after resolving both.

    Checked before every deletion. A path that resolves outside the instrumented
    directory is refused rather than deleted, so a mistake in how the directory
    was resolved costs a failed run instead of an ordinary build directory.
    """
    try:
        child.resolve().relative_to(parent.resolve())
    except ValueError:
        return False
    return True


def drop_invalid_profraws(directory: Path) -> tuple[int, int]:
    """Delete every profile buffer too short to hold a raw profile header.

    Returns (checked, dropped). One unreadable buffer otherwise fails the entire
    merge with "no profile can be merged", after every test has already passed,
    and leaves the reader with an exit code and nothing to act on.

    A dropped buffer means one test's coverage is missing from the report, so it
    is a fact to print rather than a reason to lose the verdict, and the counts
    are printed whether or not anything was dropped: the rate of this is not
    measured, so a run that reports nothing cannot be distinguished from a run
    where the scan did not happen.

    Only the top level is scanned, because that is where the merge tool reads
    from. A buffer nested deeper is not part of this merge, so deleting one would
    be damage in exchange for nothing.
    """
    checked = 0
    dropped = 0
    for path in sorted(directory.glob("*.profraw")):
        checked += 1
        try:
            size = path.stat().st_size
        except OSError as error:
            raise RunError(f"cannot read the profile buffer {path}: {error}") from error
        if size >= RAW_PROFILE_HEADER_BYTES:
            continue
        if not _inside(path, directory):
            raise RunError(
                f"refusing to delete {path}: it does not resolve inside the "
                f"instrumented build directory {directory}"
            )
        path.unlink()
        dropped += 1
        print(
            f"scoped-runner: dropped unreadable profile buffer {path} "
            f"({size} bytes, under the {RAW_PROFILE_HEADER_BYTES}-byte raw "
            f"profile header); that test's coverage is missing from this report"
        )
    print(
        f"scoped-runner: profile buffers checked {checked}, dropped {dropped} "
        f"(shorter than the {RAW_PROFILE_HEADER_BYTES}-byte raw profile header)"
    )
    return checked, dropped


def _object_names(packages: Sequence[dict[str, Any]]) -> set[str]:
    """Every name an executable of these packages can carry.

    A package's executables are named after its TARGETS as well as after the
    package itself, so the package names alone are not enough: an integration
    test target called `mcp_principal_binding` produces an executable no package
    name matches. Target names come from the same `cargo metadata` read the
    package names do, rather than from a second source.
    """
    names: set[str] = set()
    for package in packages:
        names.add(package["name"])
        names.update(package.get("targets", ()))
    return names


def _object_pattern(names: Collection[str]) -> re.Pattern[str]:
    """The shape an instrumented executable's stem has, from those names.

    Mirrors the tool's own rule rather than inventing a simpler one: an optional
    `lib` prefix, the name, and an optional trailing hash, with a dash in a name
    also matching an underscore. That last half is not optional here, because
    cargo writes the package `clio-store`'s test binary as `clio_store-<hash>`
    and a rule that only matched the dash form would delete it.

    An empty name set is refused here rather than at the caller. The pattern it
    would produce matches only the empty stem, so a caller that reached the
    pruning with nothing selected would delete every object it could see: the
    degenerate pattern is the keep set, and a keep set that matches nothing is the
    most destructive value this function could return. The caller already returns
    early on an empty selection, so refusing here makes the destructive path
    unreachable in its own right rather than one guard away from live.
    """
    if not names:
        raise RunError(
            "the pruning rule was built with no package names, so its keep "
            "pattern would match nothing and every instrumented object it can see "
            "would be deleted. Refusing rather than deleting a build cache."
        )
    alternates = "|".join(re.escape(name).replace(r"\-", "(-|_)") for name in sorted(names))
    return re.compile(rf"^(lib)?({alternates})(-[0-9a-f]+)?$")


def prune_foreign_objects(directory: Path, selected: Sequence[str]) -> int:
    """Delete instrumented executables no selected package owns. Returns the count.

    The tool builds a report's object list by walking the whole instrumented
    directory and keeping every executable whose stem matches a workspace name,
    and it merges them by union. A package filter does not narrow that walk, so a
    scoped run otherwise merges binaries an earlier run of other packages left
    behind. Their union moves a file's line set and function count, which makes
    the file's denominator a property of the build rather than of the file, and
    shows up as a merge warning about functions with mismatched data.

    The rule is name-based and therefore imperfect, which is acceptable here for
    one reason: a file this pruning wrongly removes is a file the report cannot
    contain, and the gate already reports such a file as NOT MEASURED rather than
    as a pass. A wrong prune is a loud failure; a wrong keep is a wrong number.

    The cost is a rebuild. After a run that selected three packages of seventeen,
    the other fourteen have no instrumented binaries left, so a later run needing
    them pays to build them again. That is the price of a merge that sees only
    what this run produced.
    """
    packages = workspace_packages()
    keep = _object_pattern(_object_names([p for p in packages if p["name"] in set(selected)]))
    # The whole workspace, not the selection: a binary belonging to a package this
    # run did not select is exactly what has to go, so the rule has to recognise
    # it in order to delete it.
    visible = _object_pattern(_object_names(packages))
    dropped = 0
    checked = 0
    for path in _instrumented_files(directory):
        if not visible.match(path.stem):
            continue
        checked += 1
        if keep.match(path.stem):
            continue
        if not _inside(path, directory):
            raise RunError(
                f"refusing to delete {path}: it does not resolve inside the "
                f"instrumented build directory {directory}"
            )
        path.unlink()
        dropped += 1
    print(
        f"scoped-runner: instrumented objects checked {checked}, deleted {dropped} "
        f"(executables of packages this run did not select)"
    )
    return dropped


def _instrumented_files(directory: Path) -> Iterable[Path]:
    """Every file under the instrumented directory the tool could treat as an object.

    The directory skips are the tool's own, so a walk here and a walk there see
    the same set. The executable test and the suffix list are the tool's too: a
    `.rlib` whose stem matches a package name is not an object to the merge, and
    deleting it would force a rebuild for no gain.
    """
    for parent, dirnames, filenames in os.walk(directory):
        dirnames[:] = [name for name in dirnames if name not in SKIPPED_BUILD_DIRS]
        for filename in filenames:
            if filename.endswith(NOT_OBJECT_SUFFIXES):
                continue
            path = Path(parent) / filename
            try:
                mode = path.stat().st_mode
            except OSError:
                continue
            if not stat.S_ISREG(mode):
                continue
            if os.name == "posix" and mode & 0o111 == 0:
                continue
            yield path


# --- one package, under a directory of its own ----------------------------------


class PackageMeasurement(NamedTuple):
    """One package's report, and the verdict of the test run behind it.

    The two travel together because a red suite must not cost the report: this
    mode judges nothing, so the report is the output while the run's own status
    is the exit code, and either alone would lose half of what happened.
    """

    report: Path
    test_status: int


def package_report_path(name: str) -> Path:
    """A report path of this package's own, named after the package.

    Separate per package for the same reason every other path here is: one shared
    name would let the last run's report stand in for the others. Separate from
    the gate's own paths too, so a diagnostic can never overwrite the artifact a
    floor was judged against.
    """
    return COV_DIR / f"coverage-package-{name}.json"


def package_target_override(name: str) -> str:
    """This package's instrumented directory, relative to the workspace root.

    Relative on purpose. The tool joins an override onto its own current
    directory, and this file runs every command from the workspace root, so a
    relative value and the absolute one resolve identically and a stored path
    stays readable on another machine.
    """
    return f"target/{PACKAGE_COV_DIR_NAME}/{name}"


def package_env(name: str) -> dict[str, str]:
    """The environment this package's run builds from.

    The target directory is the override and the build directory is removed. Both
    halves are the tool's rule: with the target directory set and no build
    directory, it uses the same path for both. An inherited build directory would
    send the executables somewhere else from the buffers, and the two steps in
    between would then inspect a directory this run never wrote.
    """
    env = hermetic_env()
    env[CARGO_LLVM_COV_TARGET_KEY] = package_target_override(name)
    env.pop(CARGO_LLVM_COV_BUILD_KEY, None)
    return env


def run_package_coverage(name: str) -> PackageMeasurement:
    """Measure one package into its own instrumented directory and report.

    The same two steps as the gate, in the same order, with the same buffer and
    object hygiene between them, and under a directory this package alone builds
    into. That last part is the difference from running the coverage tool's own
    per-package command: that one builds into the shared instrumented directory
    and merges whatever the object walk finds there, so an earlier run's binaries
    for other packages change this package's per-file rows.

    A red suite does not cost the report, which is the output this mode exists
    for. The test step writes no report of its own, so the buffers of every test
    that ran are on disk when it ends, and the hygiene pass and the report step
    read them exactly as they do after a green run. The run's own status is
    returned beside the report because exit 0 would say the package measured
    clean; nothing is judged here, so the status is the run's and not a verdict.
    """
    env = package_env(name)
    report_path = package_report_path(name)
    clear_profile_buffers(env)
    test_status = EXIT_OK
    try:
        run_coverage_tests([name], None, env)
    except RunError as error:
        # `_run_or_raise` already reprinted the run's own output; the message is
        # repeated here so the log still says which step failed, and the buffers
        # the failed run left are this report's input.
        print(f"scoped-runner: error: {error}", file=sys.stderr)
        test_status = EXIT_FAIL

    # The same boundary the gate owns, against the directory this run built into.
    directory = instrumented_dir(env)
    drop_invalid_profraws(directory)
    prune_foreign_objects(directory, [name])

    write_coverage_report(report_path, env)
    return PackageMeasurement(report_path, test_status)


def coverage_package_mode(name: str) -> int:
    """Measure one package and print its rows. Judges nothing, gates nothing.

    A diagnostic, and named as one. It exists because the tool's own per-package
    command reports per-file rows that are not what they say: the merge sees
    binaries belonging to other packages, and a union of objects moves a file's
    denominator. This fixes the merge and adds no verdict, so a low number here
    is a measurement to look at, not a failure and not a pass.

    A red suite still gets its report: the run step writes no report of its own,
    so the buffers of every test that ran are on disk, and the mode writes the
    report from them, prints its path, and ends with the run's own non-zero
    status rather than an absence. Only a report this run could not produce at
    all is a run error.

    The cost is stated rather than discovered: each package gets its own cold
    instrumented build the first time it is measured, because the ordinary warm
    build lives in a different directory and cannot be reused. It is under
    `target/`, which is already build output.
    """
    package = next((p for p in workspace_packages() if p["name"] == name), None)
    if package is None:
        print(
            f"scoped-runner: error: {name} is not a workspace package, so there "
            f"is nothing to measure. Workspace packages: "
            f"{', '.join(sorted(p['name'] for p in workspace_packages()))}",
            file=sys.stderr,
        )
        return EXIT_USAGE
    print(
        f"scoped-runner: measuring package {name} in its own instrumented "
        f"directory {package_target_override(name)}"
    )
    print(
        "scoped-runner: this mode judges nothing and gates nothing; its numbers "
        "are only as good as its object list, which is why the run prunes it"
    )
    try:
        measurement = run_package_coverage(name)
        doc = load_report(measurement.report)
    except RunError as error:
        print(f"scoped-runner: error: {error}", file=sys.stderr)
        return EXIT_RUN_ERROR
    rows = report_rows(doc)
    # A report holds every file any object contributed, and the objects here are
    # this package's. A file from another package would mean the directory did
    # not do its job, so it is printed rather than filtered out of the listing.
    prefix = f"{package['dir']}/"
    owned = sorted(rel for rel in rows if rel.startswith(prefix))
    print(f"scoped-runner: {len(owned)} file(s) of {name} in {measurement.report}")
    for rel in owned:
        row = rows[rel]
        print(
            f"scoped-runner: {rel} lines {row.lines_pct:.2f}% "
            f"functions {row.functions_pct:.2f}%"
        )
    strays = sorted(rel for rel in rows if not rel.startswith(prefix))
    if strays:
        print(
            f"scoped-runner: WARNING {len(strays)} reported file(s) belong to no "
            f"object of {name}, so this run's directory did not isolate it: "
            f"{', '.join(strays[:5])}",
            file=sys.stderr,
        )
    print(
        f"scoped-runner: no floor was compared and no verdict is reported; "
        f"{REPORT.name} is the gate's report and was not written"
    )
    if measurement.test_status != EXIT_OK:
        # The report covers every test that ran (the ci profile runs them all),
        # and the non-zero status is the run's own. Saying so keeps the exit code
        # from reading as a failure to produce the measurement.
        print(
            "scoped-runner: the instrumented test run failed, so this report is "
            "the output of a red suite and the mode ends with the run's status",
            file=sys.stderr,
        )
    return measurement.test_status


# --- entry point ---------------------------------------------------------------


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="Targeted tests and coverage for the files a change touches."
    )
    parser.add_argument("--base", default="master", help="ref to diff against (default: master)")
    parser.add_argument("--all", action="store_true", help="measure every package")
    parser.add_argument("--plan", action="store_true", help="print the selection and run nothing")
    parser.add_argument(
        "--tests-only",
        action="store_true",
        help="run the selected packages' tests without instrumentation, and judge nothing",
    )
    parser.add_argument(
        "--scope",
        choices=("crate", "file"),
        default="crate",
        help=(
            "crate: run every selected package's tests. "
            "file: run only the changed files' own tests. Both judge the same "
            f"{LINE_FLOOR:.0f}%% line floor (default: crate)"
        ),
    )
    parser.add_argument(
        "--self-test", action="store_true", help="run this script's own tests"
    )
    parser.add_argument(
        "--coverage-package",
        default=None,
        metavar="NAME",
        help=(
            "measure one package under an instrumented directory of its own and "
            "print its per-file rows. Compares nothing, judges nothing"
        ),
    )
    args = parser.parse_args(argv)
    if args.self_test:
        from scoped_selftest import self_test

        return self_test()

    if args.coverage_package is not None:
        return coverage_package_mode(args.coverage_package)

    try:
        packages, files, changed = select(args.base, args.all)
    except RunError as error:
        print(f"scoped-runner: error: {error}", file=sys.stderr)
        return EXIT_RUN_ERROR

    label = "test" if args.tests_only else "judge"
    codeless = parse_codeless(changed)
    no_code = [rel for rel in changed if rel in codeless and is_present(rel)]
    print(f"scoped-runner: scope={args.scope}")
    print(f"scoped-runner: {len(files)} changed file(s) to {label}")
    if no_code:
        # Printed rather than silently dropped: the gate's contract is that the
        # reader can see every file the run did not judge. The source decided it
        # holds no counted line, so the run names it and moves on.
        for rel in no_code:
            print(f"scoped-runner: NO EXECUTABLE CODE {rel}")
    print(f"scoped-runner: {len(packages)} package(s) to run: {', '.join(packages) or '(none)'}")

    file_filter = ""
    if args.scope == "file":
        # A deleted file owns no tests because nothing is left of it, and the
        # ownership lookup cannot reach a path that no crate root holds any more.
        # It is skipped and named rather than raised on, so a change that retires
        # a test module can still be measured for the files that remain.
        for rel in [r for r in changed if not is_present(r)]:
            print(f"scoped-runner: SKIPPED DELETED {rel}")
        try:
            # The filter reads `changed`, not `files`, so a test under a `tests/`
            # folder still runs even though it can never be measured. Judging
            # below reads `files`, which excludes it. Splitting the two here is
            # what keeps a new integration test from being run by nothing. The
            # deleted paths are the one thing filtered out, because asking the
            # lookup about a file that is gone is the failure this avoids.
            file_filter, unplanned = file_scope_filter([r for r in changed if is_present(r)])
        except OwnedLookupError as error:
            print(f"scoped-runner: error: {error}", file=sys.stderr)
            return EXIT_RUN_ERROR
        # A path-excluded file is not a file that owes the codebase a test module.
        # Complaining about one would fail the run for a file the report could
        # never hold in the first place, which is the failure being fixed here.
        unplanned = [rel for rel in unplanned if is_measureable(rel)]
        for rel in unplanned:
            print(f"scoped-runner: NO OWN TESTS {rel}", file=sys.stderr)
        if not file_filter:
            # Every changed file owns no test, so there is nothing to run. Saying
            # so is the honest outcome; a filter matching nothing would exit
            # non-zero and look like a test failure.
            print("scoped-runner: no changed file owns a test; nothing to run")
            if args.tests_only or not files:
                # Nothing to run and nothing to judge. Every changed file is
                # path-excluded, holds no executable code, or is gone from the
                # tree, so there is no verdict to give. Exiting non-zero
                # here reported a failure over zero files, which is the empty-scope
                # lie the run refuses to tell elsewhere.
                return EXIT_OK
            # Judging mode still has a verdict to give: these files are at 0%.
            for rel in files:
                print(f"scoped-runner: BELOW FLOOR {rel} lines 0.00%")
            return EXIT_FAIL
        print(f"scoped-runner: file-scoped filter over {len(changed)} file(s)")

    if args.plan:
        for rel in files:
            print(f"  {label} {rel}")
        if file_filter:
            print(f"  filter {file_filter}")
        return EXIT_OK

    # Nothing to measure and nothing to judge. Reaching here means the tree is
    # clean of Rust changes: `select` raises when the tree is dirty but no Rust
    # file was found, so an empty list here is a real all-clear rather than a
    # selection that silently failed.
    if not packages:
        print("scoped-runner: no changed Rust file; nothing to run")
        return EXIT_OK

    if args.tests_only:
        # A changed file whose tests cannot be located is a failure in this mode
        # too. Returning 0 here reported a green fast loop on a change whose only
        # new test never ran, which is the exact scenario the crate scope was
        # patched for earlier: a commit that adds a test must run it.
        if args.scope == "file" and unplanned:
            for rel in unplanned:
                print(
                    f"scoped-runner: FAIL {rel} owns no test module, so no test of "
                    f"its own ran; migrate its tests to the convention",
                    file=sys.stderr,
                )
            print(
                f"scoped-runner: {len(unplanned)} changed file(s) own no test",
                file=sys.stderr,
            )
            return EXIT_FAIL
        # nextest's exit code is the verdict. A failure is reported by the test
        # runner itself, with its own name and output, so this tool does not
        # summarise or reinterpret it.
        code = run_tests(packages, file_filter or None)
        if code != 0:
            return code
        return EXIT_OK

    try:
        if args.scope == "file":
            # One instrumented pass per changed file, so each file's coverage is
            # produced by its own tests and nothing else. The judgement is the
            # same one, at the same number; only the test set differs.
            offenders, unreported = judge_file_scope(packages, files, LINE_FLOOR)
            print(
                f"scoped-runner: judged {len(files)} file(s) at "
                f"{LINE_FLOOR:.2f}% lines, each from its own tests"
            )
            if offenders or unreported:
                # Named before the count, because a failing run that cannot be
                # acted on from its own log has not reported anything useful.
                report_verdicts(offenders, unreported, sys.stderr)
                print(
                    f"scoped-runner: {len(offenders)} below floor, "
                    f"{len(unreported)} not measured",
                    file=sys.stderr,
                )
                return EXIT_FAIL
            print(
                "scoped-runner: every judged file meets the floor from its own "
                f"tests ({len(files)} judged, {len(changed) - len(files)} "
                "excluded from coverage: path-excluded by the tool, or holding "
                "no executable code)"
            )
            return EXIT_OK

        run_coverage(packages, file_filter or None)
        doc = load_report(REPORT)
        rows = report_rows(doc)
    except RunError as error:
        print(f"scoped-runner: error: {error}", file=sys.stderr)
        return EXIT_RUN_ERROR

    offenders, unreported = judge(rows, files, LINE_FLOOR)

    measured = len(files) - len(unreported)
    print(f"scoped-runner: {measured}/{len(files)} changed file(s) present in the report")
    # A file this change only moved is judged exactly as any other changed file
    # is. This line says so, because a reader who renamed a file and then read a
    # coverage verdict for it would otherwise have no way to tell a file whose
    # content this change left alone from one it never touched.
    try:
        moved = renames(args.base)
    except RunError:
        moved = set()
    for rel in files:
        if rel in moved:
            print(
                f"scoped-runner: MOVED WITHOUT EDIT {rel}: this change renamed it "
                "and left its content alone, so it adds no line. Its coverage is "
                "whatever it was before the move, and it is judged on that"
            )
    report_verdicts(offenders, unreported, sys.stdout)

    if offenders or unreported:
        print(
            f"scoped-runner: {len(offenders)} below floor, {len(unreported)} not "
            f"measured, at {LINE_FLOOR:.2f}% lines",
            file=sys.stderr,
        )
        return EXIT_FAIL
    print(
        f"scoped-runner: every measured changed file reached {LINE_FLOOR:.2f}% lines"
    )
    return EXIT_OK


if __name__ == "__main__":
    # The self-test modules import this file by name. Run as a script it is `__main__`, so
    # register it under its own name, or they would load a second copy and test that one.
    sys.modules.setdefault("scoped_runner", sys.modules[__name__])
    sys.exit(main(sys.argv[1:]))
