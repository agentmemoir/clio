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
  number applies to it. Those are listed in `scripts/coverage_codeless.json` and
  the run names each one rather than inventing a denominator of zero.

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
CODELESS_PATH = Path(__file__).resolve().parent / "coverage_codeless.json"

sys.path.insert(0, str(Path(__file__).resolve().parent))
try:
    import owned_tests as owned
except ImportError as error:  # pragma: no cover - only when the file is missing
    print(f"scoped-runner: cannot import scripts/owned_tests.py: {error}", file=sys.stderr)
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

# The key in the codeless file that records files holding no executable code.
#
# A file whose items are all declarations, `use`s, or `const` values compiles to
# no instrumented region, so no llvm-cov report can ever contain it. That is true
# of the line count as well as of the function count, so the gate, which judges
# lines, cannot judge such a file either. Holding one to a coverage floor is not a
# measurement: the run reports every measured file above its floor and still exits
# non-zero, which is what stopped any commit that adds a `mod` to a crate root.
#
# Recorded rather than derived, because the property cannot be read off the
# report. A file with no instrumented region is absent, and absence is also
# exactly what a scope gap looks like, so the two cannot be told apart from the
# report alone. Deciding by reading the source would mean modelling enough Rust
# to know which items emit a counted region and which do not, and being wrong in
# the permissive direction would turn an unmeasured file into a silent pass. A
# recorded list is wrong in the other direction: a new such file fails until one
# line is added, which is loud, visible, and self-correcting. A file holding a
# function is judged normally, so a crate root that is mostly re-exports but
# defines one is absent from this list on purpose.
CODELESS_KEY = "_no_executable_code"

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


def load_codeless() -> dict[str, str]:
    """The recorded files that hold no executable code, mapped to why.

    A missing file is an empty list, not an error: a checkout without one simply
    has nothing recorded, which is the same state as recording no such file, and
    both mean every measured file is judged.
    """
    if not CODELESS_PATH.exists():
        return {}
    try:
        doc = json.loads(CODELESS_PATH.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise RunError(f"cannot read {CODELESS_PATH}: {error}") from error
    recorded = doc.get(CODELESS_KEY, {})
    if not isinstance(recorded, dict):
        raise RunError(f"{CODELESS_PATH}: {CODELESS_KEY!r} must be an object")
    out: dict[str, str] = {}
    for rel, reason in recorded.items():
        if not isinstance(rel, str) or not isinstance(reason, str):
            raise RunError(
                f"{CODELESS_PATH}: every {CODELESS_KEY!r} entry maps a path to a "
                f"reason string, and {rel!r} does not"
            )
        # An underscore key is a note beside the list.
        if rel.startswith("_"):
            continue
        out[rel] = reason
    return out


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
    judgeable = [f for f in files if is_judgeable(f, load_codeless()) and is_present(f)]
    packages = workspace_packages()
    if everything:
        return [p["name"] for p in packages], judgeable, files
    if not files:
        # A genuinely clean tree: no changed file of any type.
        return [], judgeable, files
    # Every file a report could hold, codeless ones included. The check is about
    # belonging to a package at all, which has nothing to do with whether the file
    # has code, so it must not lose the recorded files the judging set just dropped.
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
        return self_test()

    if args.coverage_package is not None:
        return coverage_package_mode(args.coverage_package)

    try:
        packages, files, changed = select(args.base, args.all)
    except RunError as error:
        print(f"scoped-runner: error: {error}", file=sys.stderr)
        return EXIT_RUN_ERROR

    label = "test" if args.tests_only else "judge"
    codeless = load_codeless()
    no_code = [rel for rel in changed if rel in codeless]
    print(f"scoped-runner: scope={args.scope}")
    print(f"scoped-runner: {len(files)} changed file(s) to {label}")
    if no_code:
        # Printed rather than silently dropped: the gate's contract is that the
        # reader can see every file the run did not judge. The line says why it
        # was not judged, and the recorded reason says who decided that.
        for rel in no_code:
            print(f"scoped-runner: NO EXECUTABLE CODE {rel}: {codeless[rel]}")
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
                # path-excluded, recorded as holding no executable code, or gone
                # from the tree, so there is no verdict to give. Exiting non-zero
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
                "excluded from coverage: path-excluded by the tool, or recorded "
                "as holding no executable code)"
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

    # A directory-to-package map in which the names deliberately differ, pinning
    # the property that the directory name is never used on its own: handing it
    # to `cargo llvm-cov -p` gives a value that does not resolve. Every directory
    # in this workspace matches its package name, so the mismatch here is
    # synthetic. The dirs are repository-relative because that is the form
    # `workspace_packages` stores and the form git reports.
    fake = [
        {"name": "example-crate", "deps": set(), "dir": "crates/mismatched-dir"},
        {"name": "clio-store", "deps": set(), "dir": "crates/clio-store"},
    ]
    check(
        "a leaf file belongs to its crate",
        package_owners("crates/clio-store/src/sqlite.rs", fake),
        ["clio-store"],
    )
    check(
        "a crate whose directory name differs maps to its package name",
        package_owners("crates/mismatched-dir/src/lib.rs", fake),
        ["example-crate"],
    )
    check(
        "a nested test file belongs to its crate",
        package_owners("crates/mismatched-dir/tests/harness/mod.rs", fake),
        ["example-crate"],
    )
    check(
        "a file outside crates belongs to nothing",
        package_owners("scripts/scoped_runner.py", fake),
        [],
    )
    check("test-only files are not measured", is_measureable("crates/clio/src/a_tests.rs"), False)
    check("production files are measured", is_measureable("crates/clio/src/a.rs"), True)
    # A file holding no executable code is a third exclusion, and the two halves
    # it could be confused with are pinned here too. `is_measureable` alone cannot
    # see it, which is why the recorded set is a separate argument: without one,
    # every commit adding a `mod` to a crate root failed a gate that reported zero
    # files below its floor.
    check(
        "a file holding no executable code is not judged",
        is_judgeable("crates/clio-types/src/lib.rs", {"crates/clio-types/src/lib.rs"}),
        False,
    )
    check(
        "the same file is judged when it is not recorded",
        is_judgeable("crates/clio-types/src/lib.rs"),
        True,
    )
    check(
        "the recorded set is read from the codeless file",
        "crates/clio-types/src/lib.rs" in load_codeless(),
        True,
    )
    check(
        "a crate root that does define functions stays judged",
        is_judgeable("crates/clio/src/lib.rs"),
        True,
    )
    # A deleted file is a changed file with nothing left to measure. Left in the
    # judged list it was reported as a file the run failed to measure, so a change
    # that only retired a file failed a gate that had nothing to say about it.
    check("a file in the tree is present", is_present("crates/clio-types/src/civil.rs"), True)
    check(
        "a file the change deleted is not present",
        is_present("crates/clio-types/src/retired.rs"),
        False,
    )
    # A count alone is not a diagnosis. Both scopes name every file they could not
    # pass, because a reader holding only "22 below floor" has to diff the printed
    # pass list against the judged list to learn which 22 those were.
    verdict_log = io.StringIO()
    report_verdicts(
        ["crates/a/src/low.rs lines 71.00% (floor 80.00)"],
        ["crates/a/src/gone.rs"],
        verdict_log,
    )
    printed = verdict_log.getvalue()
    check("a failing run names the file below the floor", "crates/a/src/low.rs" in printed, True)
    check("a failing run names the file it could not measure", "gone.rs" in printed, True)

    # Nothing to run and nothing to judge is an all-clear. A change whose only
    # Rust file was deleted leaves the judged list empty, and failing over it
    # would report a failure with no file to point at. And a failing file-scoped
    # run must name its offenders, which it did not: it printed the count alone.
    original_select = globals()["select"]
    original_filter = globals()["file_scope_filter"]
    original_file_judge = globals()["judge_file_scope"]
    real_stderr = sys.stderr
    try:
        globals()["select"] = lambda base, everything: (
            [],
            [],
            ["crates/clio-types/src/retired.rs"],
        )
        globals()["file_scope_filter"] = lambda files: ("", [])
        check(
            "no judgeable file left to measure is an all-clear",
            main(["--scope", "file", "--base", "master"]),
            EXIT_OK,
        )
        globals()["select"] = lambda base, everything: (
            ["clio-types"],
            ["crates/clio-types/src/civil.rs"],
            ["crates/clio-types/src/civil.rs"],
        )
        globals()["file_scope_filter"] = lambda files: (
            "",
            ["crates/clio-types/src/civil.rs"],
        )
        check(
            "a judgeable file that owns no test is still a failure",
            main(["--scope", "file", "--base", "master"]),
            EXIT_FAIL,
        )
        globals()["file_scope_filter"] = lambda files: ("test(/^x(::|$)/)", [])
        globals()["judge_file_scope"] = lambda packages, files, floor: (
            ["crates/clio-types/src/civil.rs lines 71.00% (floor 80.00)"],
            ["crates/clio-types/src/vanished.rs"],
        )
        captured = io.StringIO()
        sys.stderr = captured
        try:
            code = main(["--scope", "file", "--base", "master"])
        finally:
            sys.stderr = real_stderr
        check("a failing file-scoped run exits non-zero", code, EXIT_FAIL)
        check(
            "the offender is named with its percentage",
            "civil.rs lines 71.00% (floor 80.00)" in captured.getvalue(),
            True,
        )
        check(
            "the file the report could not hold is named",
            "NOT MEASURED crates/clio-types/src/vanished.rs" in captured.getvalue(),
            True,
        )
    finally:
        sys.stderr = real_stderr
        globals()["select"] = original_select
        globals()["file_scope_filter"] = original_filter
        globals()["judge_file_scope"] = original_file_judge

    # The two halves of the tool's exclusion, pinned separately. The suffix half
    # alone let every integration test be judged, and llvm-cov can never report
    # one, so each such change failed a gate it had already passed on coverage.
    check(
        "an integration test is not measured",
        is_measureable("crates/clio-mcp/tests/mcp_principal_binding.rs"),
        False,
    )
    check(
        "a module under a tests directory is not measured",
        is_measureable("crates/clio-mcp/tests/binding_support/mod.rs"),
        False,
    )
    check(
        "a tests directory inside src is not measured either",
        is_measureable("crates/clio-store/src/tests/mod.rs"),
        False,
    )
    check(
        "an examples or benches directory is not measured",
        (
            is_measureable("crates/clio/examples/demo.rs"),
            is_measureable("crates/clio/benches/parse.rs"),
        ),
        (False, False),
    )
    check(
        "a production file whose name merely contains tests is measured",
        (
            is_measureable("crates/clio/src/tests_helper.rs"),
            is_measureable("crates/clio/src/latest.rs"),
        ),
        (True, True),
    )

    # Untracked files are read with `ls-files`, not `status`, and the reason is
    # the one case `status` cannot express. A wholly untracked directory collapses
    # to a single entry ending in a slash, so a real git repository is the only
    # way to pin this: a mock would agree with whichever implementation it was
    # written against. The stub below reproduces exactly what git prints.
    original_git = globals()["git"]
    original_merge_base_for_untracked = globals()["merge_base"]
    try:
        globals()["merge_base"] = lambda base: "0" * 40

        def fake_git(*args: str) -> str:
            if args[:2] == ("ls-files", "--others"):
                # What `ls-files -z` prints for the same tree `status` collapses
                # into one entry: a path per file, NUL-separated.
                return "crates/x/src/lib.rs\0crates/x/Cargo.toml\0"
            return ""

        globals()["git"] = fake_git
        untracked = changed_files("master")
    finally:
        globals()["git"] = original_git
        globals()["merge_base"] = original_merge_base_for_untracked
    check(
        "a Rust file under a wholly untracked directory is seen",
        untracked,
        ["crates/x/src/lib.rs"],
    )

    # The tests-only mode must be a plain nextest run with no instrumentation
    # flags and no report, because the whole point of it is to skip the two
    # expensive steps. A stray --json or llvm-cov here would silently turn the
    # fast loop back into the slow one.
    recorded: dict[str, list[str]] = {}

    class _Done:
        returncode = 0

    original_run = globals()["subprocess"].run

    def fake_run(cmd, **kw):
        recorded["cmd"] = list(cmd)
        return _Done()

    try:
        globals()["subprocess"].run = fake_run
        code = run_tests(["a", "b"])
    finally:
        globals()["subprocess"].run = original_run
    cmd = recorded["cmd"]
    check("tests-only runs nextest", cmd[:3], ["cargo", "nextest", "run"])
    check("tests-only passes both packages", cmd[3:7], ["-p", "a", "-p", "b"])
    check("tests-only is not instrumented", "llvm-cov" in cmd, False)
    check("tests-only exports no report", "--json" in cmd, False)
    check("tests-only keeps the warm build", "--no-clean" in cmd, False)
    check("tests-only returns nextest's exit code", code, 0)
    check("tests-only uses the ci profile", "--profile" in cmd and "ci" in cmd, True)
    check("the crate scope sends no filter", "-E" in cmd, False)

    # The file scope sends one filter expression. Two invocations would run the
    # overlap twice, so the count of `-E` flags is asserted rather than assumed.
    try:
        globals()["subprocess"].run = fake_run
        run_tests(["a"], "test(/^foo::tests(::|$)/)")
    finally:
        globals()["subprocess"].run = original_run
    cmd = recorded["cmd"]
    check("the file scope sends exactly one filter", cmd.count("-E"), 1)
    check("the filter follows its expression", cmd[cmd.index("-E") + 1], "test(/^foo::tests(::|$)/)")

    # A filter expression built from real module paths must be anchored, because
    # nextest's `test()` is an unanchored substring match: `foo::tests` would
    # otherwise also select `other::foo::tests`.
    check(
        "an expression anchors the module path at both ends",
        owned.filter_expression(["foo::tests"]) == "test(/^foo::tests(::|$)/)",
        True,
    )
    union = owned.filter_expression(["a::tests", "b::tests"])
    check("a union is wrapped in one expression", union.startswith("(") and union.endswith(")"), True)
    check("a union has one branch per module", union.count("test("), 2)
    check("no modules yields no expression", owned.filter_expression([]), "")

    # The one rule, in the one judge both scopes call. A file below the floor
    # fails whatever its function coverage says, which is the whole of what the
    # function half of a row is for: it is printed, and nothing compares it.
    thin_offenders, thin_missing = judge(
        {"crates/a/src/thin.rs": CoverageRow(71.0, 100.0)},
        ["crates/a/src/thin.rs"],
    )
    check("a file at 71 lines fails at 100 functions", len(thin_offenders), 1)
    check("a file at 71 lines is named", "thin.rs" in thin_offenders[0], True)
    check("nothing is unreported when the report holds the file", thin_missing, [])
    fine_offenders, _ = judge(
        {"crates/a/src/fine.rs": CoverageRow(91.8, 40.0)}, ["crates/a/src/fine.rs"]
    )
    check("a file at 91.8 lines passes at 40 functions", fine_offenders, [])
    _, fixed_missing = judge({}, ["crates/a/src/gone.rs"])
    check("an unmeasured file is reported, not passed", fixed_missing, ["crates/a/src/gone.rs"])
    check(
        "the floor is the one number in this file, and it is 80",
        LINE_FLOOR,
        80.0,
    )
    check(
        "a file one line under the floor still fails, so nothing rounds in its favour",
        len(judge({"crates/a/src/x.rs": CoverageRow(79.99, 100.0)}, ["crates/a/src/x.rs"])[0]),
        1,
    )
    check(
        "a file exactly at the floor passes, and nothing else is consulted",
        judge({"crates/a/src/x.rs": CoverageRow(80.0, 0.0)}, ["crates/a/src/x.rs"])[0],
        [],
    )

    # The fail-closed paths. Each one is a way the tool can end up measuring
    # nothing, and each must be an error rather than a clean run. These are the
    # cases that matter most, because an empty scope is indistinguishable from
    # a pass in the output.
    class Boom(RunError):
        pass

    def raises(fn, *a, **kw) -> bool:
        try:
            fn(*a, **kw)
        except RunError:
            return True
        return False

    original_ref_exists = globals()["_ref_exists"]
    original_merge_base = globals()["merge_base"]
    original_changed = globals()["changed_files"]
    try:
        globals()["_ref_exists"] = lambda ref: False
        globals()["merge_base"] = lambda base: None
        check("a ref that does not exist is an error", raises(select, "no-such-ref", False), True)

        # The case that broke CI. A checkout in detached HEAD mode has no local
        # branch for the base, only refs/remotes/origin/<base>, so the bare name
        # does not resolve while the remote form does. Refusing here would fail
        # every pull request; treating it as an empty scope would pass over
        # nothing. The remote form has to be found and used.
        globals()["_ref_exists"] = lambda ref: ref.startswith("origin/")
        check(
            "a bare ref that only exists as a remote-tracking ref still resolves",
            resolve_ref("master"),
            "origin/master",
        )
        check(
            "the remote fallback makes select() succeed where the bare name fails",
            raises(select, "master", False),
            False,
        )

        globals()["_ref_exists"] = lambda ref: True
        # A changed .rs file that no package owns can never be measured, so it
        # must be an error rather than a silent all-clear.
        globals()["changed_files"] = lambda base: ["stray/tool.rs"]
        check(
            "a changed Rust file no package owns is an error",
            raises(select, "master", False),
            True,
        )

        # A change with no Rust file at all is a legitimate all-clear: docs, a
        # Makefile, or a Python script cannot move a Rust line count.
        globals()["changed_files"] = lambda base: []
        globals()["workspace_packages"] = lambda: [{"name": "a", "deps": set()}]
        packages, files, changed = select("master", False)
        check("a change with no Rust file selects nothing", (packages, files, changed), ([], [], []))

        # A change that touches ONLY test-named files. llvm-cov path-excludes
        # those, so nothing is judgeable, but the tests still have to run: seeding
        # the closure from judgeable files alone selected no package here and the
        # gate would have passed a commit that added a test without ever running
        # it. Measured over 26 past bug-fix commits, this was one of two commits
        # the scoping would not have covered, and the other was a fixed one.
        globals()["workspace_packages"] = lambda: [
            {"name": "a", "dir": "crates/a", "deps": set()},
            {"name": "b", "dir": "crates/b", "deps": {"a"}},
        ]
        globals()["changed_files"] = lambda base: ["crates/a/src/thing_tests.rs"]
        packages, files, changed = select("master", False)
        check(
            "a test-only change still selects its package and its dependents",
            (packages, files),
            (["a", "b"], []),
        )
        # The same must hold for an integration test, which the suffix half alone
        # let through into the judged list. It is path-excluded, so it can never
        # be measured, but it still has to select its package so the test runs.
        globals()["changed_files"] = lambda base: ["crates/a/tests/harness.rs"]
        packages, files, changed = select("master", False)
        check(
            "an integration-test change selects its package and is not judged",
            (packages, files, changed),
            (["a", "b"], [], ["crates/a/tests/harness.rs"]),
        )

        # A file the change deleted still selects its package, so the crate's tests
        # run, but it is not judged: nothing is left in it for a report to hold. The
        # presence test reads the real tree, so the surviving half of the pair is a
        # path that exists and the other is one that cannot.
        globals()["workspace_packages"] = lambda: [
            {"name": "clio-types", "dir": "crates/clio-types", "deps": set()},
        ]
        globals()["changed_files"] = lambda base: [
            "crates/clio-types/src/civil.rs",
            "crates/clio-types/src/retired.rs",
        ]
        packages, files, changed = select("master", False)
        check(
            "a deleted file still selects its package but is not judged",
            (packages, files),
            (["clio-types"], ["crates/clio-types/src/civil.rs"]),
        )
        check(
            "a deleted file is still reported as changed",
            changed,
            ["crates/clio-types/src/civil.rs", "crates/clio-types/src/retired.rs"],
        )
    finally:
        globals()["_ref_exists"] = original_ref_exists
        globals()["merge_base"] = original_merge_base
        globals()["changed_files"] = original_changed

    # There is no per-file floor to ratchet and no aggregate to compare against.
    # Both used to be rules here, and both are gone, so the cases that pinned them
    # are replaced by the ones that hold the line in place: the same row judged at
    # the same number from the same function, whatever the caller passes.
    check(
        "the judge reads LINE_FLOOR when no floor is passed",
        judge({"crates/a/src/known.rs": CoverageRow(79.99, 100.0)}, ["crates/a/src/known.rs"])[0],
        ["crates/a/src/known.rs lines 79.99% (floor 80.00)"],
    )


    # The test-concurrency cap is a flake mitigation. If it is ever dropped by
    # accident, the shared-Postgres suites get their contention back, so its
    # presence is asserted rather than assumed.
    check("both modes cap test concurrency", TEST_THREADS, "2")

    # An integration target is selected by its Cargo target name, and its root-level
    # tests share the empty module path with every other integration target in the
    # same package. A name-only filter would therefore select all 20-odd of them, so
    # the binary predicate is load-bearing and is asserted here.
    check(
        "an integration target with no module path still gets a binary predicate",
        owned.filter_expression([], "retention_profile_harness"),
        "binary(retention_profile_harness)",
    )
    check(
        "an integration target with modules combines binary and name predicates",
        owned.filter_expression(["tests"], "conformance"),
        "binary(conformance) and test(/^tests(::|$)/)",
    )
    check(
        "an integration filter is anchored inside its target",
        owned.filter_expression(["tests"], "conformance").count("binary("),
        1,
    )

    # Per-file attribution. Two changed files must not share one report, or the
    # second file's number stands in for the first's. Measured before this was
    # fixed: create.rs read 81.82% alone and 92.12% when a second file's tests
    # joined the same run, which is its whole-crate figure.
    first = report_path_for("crates/a/src/first.rs")
    second = report_path_for("crates/a/src/second.rs")
    check("two files get two report paths", first != second, True)
    check("a report path is per file", first.name.startswith("coverage-file-"), True)
    check(
        "the same file always resolves to the same report path",
        report_path_for("crates/a/src/first.rs"),
        first,
    )
    check(
        "a path with no modules and no target yields no filter",
        owned.filter_expression([], None),
        "",
    )

    # --- the one rule: a LINE floor, and nothing else --------------------------
    #
    # The operator withdrew the function floor, and these cases pin what that
    # means at the judge in both directions: a file that would have failed on
    # functions alone now passes, and a file that fails on lines alone still
    # fails. A change that reintroduced a function comparison would leave the
    # second case passing, and a change that only removed the function fraction
    # from the message would leave the first passing for the wrong reason.
    #
    # Both scopes reach the same `judge`, so there is one of these rather than
    # two: one scope judging on functions would be a gate with two answers to the
    # same question.
    function_only = {"crates/a/src/half.rs": CoverageRow(91.0, 40.0, 100, 5, 91, 2)}
    check(
        "a file whose only shortfall is function coverage passes",
        judge(function_only, ["crates/a/src/half.rs"]),
        ([], []),
    )
    lines_only_short = {"crates/a/src/half.rs": CoverageRow(79.9, 100.0, 100, 5, 79, 5)}
    lines_offenders, _ = judge(lines_only_short, ["crates/a/src/half.rs"])
    check("a file short on lines alone still fails", len(lines_offenders), 1)
    check("the offender names the line floor it missed", "(floor 80.00)" in lines_offenders[0], True)


    # --- the two coverage steps, and what each may and may not carry ---------
    #
    # The split is the whole reason this gate can act on a run, so each flag is
    # asserted rather than assumed. A flag carried by the wrong step is a silent
    # failure in one direction or the other: a report written by the test step
    # cannot be cleaned up before the merge, and a merge inside the test step
    # cannot be cleaned up at all.
    test_cmd = coverage_test_command(["clio-types", "clio-store"], "test(/^a::tests(::|$)/)")
    report_cmd = coverage_report_command(Path("/tmp/report.json"))
    check("the test step is a nextest run", test_cmd[:3], ["cargo", "llvm-cov", "nextest"])
    check(
        "the test step names both packages",
        [test_cmd[i + 1] for i, flag in enumerate(test_cmd) if flag == "-p"],
        ["clio-types", "clio-store"],
    )
    check("the test step writes no report", "--no-report" in test_cmd, True)
    check("the test step exports no json", "--json" in test_cmd, False)
    check("the test step names no report path", "--output-path" in test_cmd, False)
    check(
        "the test step passes no failure mode",
        [flag for flag in test_cmd if flag.startswith("--failure-mode")],
        [],
    )
    # `--no-clean` is rejected together with `--no-report` by the tool itself, so
    # carrying both is a hard error before anything builds rather than a warning.
    check(
        "the test step omits the flag no-report conflicts with",
        "--no-clean" in test_cmd,
        False,
    )
    check("the test step sends exactly one filter", test_cmd.count("-E"), 1)
    check(
        "the filter follows its expression",
        test_cmd[test_cmd.index("-E") + 1],
        "test(/^a::tests(::|$)/)",
    )
    check("the report step is the report subcommand", report_cmd[:3], ["cargo", "llvm-cov", "report"])
    check("the report step exports json", "--json" in report_cmd, True)
    check(
        "the report step names the report path",
        report_cmd[report_cmd.index("--output-path") + 1],
        "/tmp/report.json",
    )
    check(
        "the report step fails only when every buffer is unreadable",
        "--failure-mode=all" in report_cmd,
        True,
    )
    # A package filter does not narrow the tool's object walk, so passing one to
    # the report step would look like it restricted something and change nothing.
    check("the report step selects no package", "-p" in report_cmd, False)
    check("the two steps are different commands", test_cmd == report_cmd, False)

    # --- one package, under an instrumented directory of its own -------------
    #
    # The point of the mode is the directory, not the package filter: the tool
    # builds a report's object list by walking the whole instrumented target
    # directory, a package selection does not narrow that walk, and the merge
    # unions what it finds. So a row read out of the shared directory is a
    # property of whatever builds happen to be lying around, and the cases below
    # pin the two things that make it not so.
    store_report = package_report_path("clio-store")
    ops_report = package_report_path("clio-ops")
    check("a package report is named after the package", store_report.name, "coverage-package-clio-store.json")
    check("two packages get two report paths", store_report != ops_report, True)
    check(
        "a package report cannot overwrite the gate's report",
        store_report == REPORT,
        False,
    )
    check(
        "a package report cannot overwrite a per-file report",
        store_report.parent == REPORT.parent and store_report != REPORT,
        True,
    )
    override = package_target_override("clio-store")
    check("the package directory is named after the package", override.endswith("/clio-store"), True)
    check(
        "the package directory is relative, so it reads the same on another machine",
        override.startswith("/"),
        False,
    )
    check(
        "the package directory is not the shared instrumented directory",
        INSTRUMENTED_DIR_NAME in override,
        False,
    )
    check(
        "the package directory is under target, which is already build output",
        override.startswith("target/"),
        True,
    )
    # The resolution has to come from the environment the run builds from, or a
    # caller that pinned the directory would be reading a different one from the
    # tool's and the two steps would inspect a directory the run never wrote.
    resolved = resolved_instrumented_dir({CARGO_LLVM_COV_TARGET_KEY: override})
    check(
        "the override in the run's environment is the directory resolved",
        resolved,
        (ROOT / override).resolve(),
    )
    # The tool uses the build directory when it is set and the target directory
    # when it is not, so an inherited one silently splits the two paths this run
    # writes to from the path it reads back.
    inherited_build = os.environ.get(CARGO_LLVM_COV_BUILD_KEY)
    try:
        os.environ[CARGO_LLVM_COV_BUILD_KEY] = "target/some-other-build"
        check(
            "an inherited build directory is dropped so the two paths cannot diverge",
            CARGO_LLVM_COV_BUILD_KEY in package_env("clio-store"),
            False,
        )
    finally:
        if inherited_build is None:
            os.environ.pop(CARGO_LLVM_COV_BUILD_KEY, None)
        else:
            os.environ[CARGO_LLVM_COV_BUILD_KEY] = inherited_build
    check(
        "the run's environment carries the package's own target directory",
        package_env("clio-store").get(CARGO_LLVM_COV_TARGET_KEY),
        override,
    )

    # A failing step has to carry its own output. An exit code alone is what left
    # a whole run's diagnosis inside a tool's message nobody printed.
    class _Failing:
        returncode = 1
        stdout = ""
        stderr = "error: invalid instrumentation profile data (file header is corrupt)"

    def failing_run(cmd, **kw):
        return _Failing()

    original_run_for_failure = globals()["subprocess"].run
    original_stderr_for_failure = sys.stderr
    failure_log = io.StringIO()
    try:
        globals()["subprocess"].run = failing_run
        sys.stderr = failure_log
        try:
            raised = raises(_run_or_raise, ["cargo", "llvm-cov", "report"], {}, "the report step failed")
        finally:
            sys.stderr = original_stderr_for_failure
    finally:
        globals()["subprocess"].run = original_run_for_failure
    check("a failing step is an error", raised, True)
    check(
        "a failing step's own output is printed",
        "file header is corrupt" in failure_log.getvalue(),
        True,
    )

    # A silent run is not evidence of a finished one. The instrumented build can
    # take minutes with nothing between the two count lines, and this repository
    # has already spent an incident on reading an empty log as a hang.
    class _Quiet:
        returncode = 0
        stdout = "\n".join(f"building line {n}" for n in range(1, 400))
        stderr = ""

    try:
        globals()["subprocess"].run = lambda cmd, **kw: _Quiet()
        tail_log = io.StringIO()
        original_stdout_for_tail = sys.stdout
        try:
            sys.stdout = tail_log
            _run_or_raise(["cargo", "llvm-cov", "report"], {}, "the report step failed")
        finally:
            sys.stdout = original_stdout_for_tail
    finally:
        globals()["subprocess"].run = original_run_for_failure
    tail_text = tail_log.getvalue()
    check("a successful run prints the tail of its own output", "building line 399" in tail_text, True)
    check("a successful run does not print its whole output", "building line 1\n" in tail_text, False)
    check("a successful run says it finished", "the run finished" in tail_text, True)

    # A profile-buffer clean that fails is the one place a stale number can get in
    # silently: the buffers survive, the merge picks them up, and the report claims
    # coverage for code that did not run. A non-zero exit is a refusal.
    class _CleanFails:
        returncode = 101
        stdout = ""
        stderr = "error: the profile data directory could not be read"

    original_run_for_clean = globals()["subprocess"].run
    try:
        globals()["subprocess"].run = lambda cmd, **kw: _CleanFails()
        clean_refused = raises(clear_profile_buffers, {})
    finally:
        globals()["subprocess"].run = original_run_for_clean
    check("a failing profile-buffer clean is an error", clean_refused, True)

    # The gate path must inspect the directory its own run built in. Resolving it
    # from the process environment instead reads whatever the shell happens to
    # hold, so a run that pins the directory would have its hygiene steps look at a
    # directory the tool never wrote. The two agree today, which is exactly why the
    # difference needs a case: the one-package mode has always passed its own
    # environment, and the gate path did not.
    step_envs: dict[str, Any] = {}

    def record_dir(env: dict[str, str] | None = None) -> Path:
        step_envs["dir"] = env
        return ROOT / "target" / "llvm-cov-target"

    original_gate_globals = {
        name: globals()[name]
        for name in (
            "clear_profile_buffers",
            "run_coverage_tests",
            "instrumented_dir",
            "drop_invalid_profraws",
            "prune_foreign_objects",
            "write_coverage_report",
        )
    }
    try:
        globals()["clear_profile_buffers"] = lambda env: step_envs.__setitem__("clear", env)
        globals()["run_coverage_tests"] = (
            lambda pkgs, filt=None, env=None: step_envs.__setitem__("test", env)
        )
        globals()["instrumented_dir"] = record_dir
        globals()["drop_invalid_profraws"] = lambda directory: step_envs.__setitem__(
            "buffers", directory
        )
        globals()["prune_foreign_objects"] = (
            lambda directory, selected: step_envs.__setitem__("prune", directory)
        )
        globals()["write_coverage_report"] = (
            lambda path, env=None: step_envs.__setitem__("report", env)
        )
        run_coverage(["clio-store"])
    finally:
        for name, value in original_gate_globals.items():
            globals()[name] = value
    check(
        "the gate path resolves its instrumented directory from its own run's "
        "environment, not the process's",
        step_envs.get("dir") is step_envs.get("test") and step_envs.get("dir") is not None,
        True,
    )
    check(
        "and both hygiene steps inspect the directory that resolution returned",
        (step_envs.get("buffers"), step_envs.get("prune")),
        (ROOT / "target" / "llvm-cov-target", ROOT / "target" / "llvm-cov-target"),
    )
    check(
        "the report step runs in the same environment as the test step",
        step_envs.get("report") is step_envs.get("test"),
        True,
    )

    # --- counts out of the report, not only percentages ----------------------
    def report_with(summary: dict[str, Any]) -> dict[str, Any]:
        return {
            "data": [
                {
                    "files": [
                        {"filename": str(ROOT / "crates/a/src/thing.rs"), "summary": summary}
                    ]
                }
            ]
        }

    measured = report_rows(
        report_with(
            {
                "lines": {"count": 43, "covered": 32, "percent": 74.42},
                "functions": {"count": 34, "covered": 23, "percent": 67.65},
            }
        )
    )
    row = measured["crates/a/src/thing.rs"]
    check(
        "a row carries the percentages first",
        (row.lines_pct, row.functions_pct),
        (74.42, 67.65),
    )
    check(
        "a row carries the denominators the percentages were computed from",
        (row.lines_count, row.functions_count),
        (43, 34),
    )
    # A missing count is a shape change, not a measurement of nothing. Dropping
    # the row would report the file as unmeasured and look like a scope gap;
    # storing zero would invent a denominator and make a percentage meaningless.
    unknown = report_rows(
        report_with(
            {
                "lines": {"percent": 74.42},
                "functions": {"percent": 67.65},
            }
        )
    )
    check("a row with percentages and no counts is kept", "crates/a/src/thing.rs" in unknown, True)
    check(
        "a missing count is unknown rather than zero",
        (unknown["crates/a/src/thing.rs"].lines_count,
         unknown["crates/a/src/thing.rs"].functions_count),
        (None, None),
    )
    # The existing skip path is unchanged: a row with no percentage cannot be
    # judged, so it is not a row at all.
    check(
        "a row with no percentage is still dropped",
        report_rows(
            report_with(
                {"lines": {"count": 43}, "functions": {"count": 34}}
            )
        ),
        {},
    )
    check(
        "a boolean is not a count",
        _count({"count": True}),
        None,
    )

    # --- the profile buffers that cannot be read -----------------------------
    #
    # The fixture is a real directory with real files, because the rule is about
    # file sizes on disk and a mock would agree with whichever implementation it
    # was written against.
    with tempfile.TemporaryDirectory(prefix="scoped-runner-profraw-") as scratch:
        profraw_root = Path(scratch)
        healthy = profraw_root / "clio-1234-5678_0.profraw"
        empty = profraw_root / "clio-1234-5678_1.profraw"
        truncated = profraw_root / "clio-1234-5678_2.profraw"
        nested_dir = profraw_root / "nested"
        nested_dir.mkdir()
        nested = nested_dir / "clio-1234-5678_3.profraw"
        healthy.write_bytes(b"\x00" * (RAW_PROFILE_HEADER_BYTES + 64))
        empty.write_bytes(b"")
        truncated.write_bytes(b"\x00" * (RAW_PROFILE_HEADER_BYTES - 1))
        nested.write_bytes(b"")
        buffer_log = io.StringIO()
        original_stdout_for_buffers = sys.stdout
        try:
            sys.stdout = buffer_log
            checked, dropped = drop_invalid_profraws(profraw_root)
        finally:
            sys.stdout = original_stdout_for_buffers
        printed = buffer_log.getvalue()
        check("every buffer at the top level is checked", checked, 3)
        check("both unreadable buffers are dropped", dropped, 2)
        check("a buffer with a full header survives", healthy.exists(), True)
        check("an empty buffer is dropped", empty.exists(), False)
        check("a buffer one byte short of the header is dropped", truncated.exists(), False)
        # Only the top level is read by the merge, so a buffer the merge never
        # sees is not this step's to delete.
        check("a nested buffer is left alone", nested.exists(), True)
        check("both dropped buffers are named by path", printed.count("dropped unreadable profile buffer"), 2)
        check("a dropped buffer's size is printed", f"({RAW_PROFILE_HEADER_BYTES - 1} bytes" in printed, True)
        check(
            "the counts are printed whether or not anything was dropped",
            "profile buffers checked 3, dropped 2" in printed,
            True,
        )
        # Nothing dropped is the common case and still has to be visible, because
        # the rate of a dropped buffer is not measured.
        clean_log = io.StringIO()
        original_stdout_for_clean = sys.stdout
        try:
            sys.stdout = clean_log
            checked_clean, dropped_clean = drop_invalid_profraws(profraw_root)
        finally:
            sys.stdout = original_stdout_for_clean
        check(
            "a run that drops nothing still prints both counts",
            (checked_clean, dropped_clean, "dropped 0" in clean_log.getvalue()),
            (1, 0, True),
        )

    # A directory that does not exist is a refusal, not "zero buffers". Reading
    # zero buffers is exactly what a wrong path produces.
    missing_override = os.environ.get("CARGO_LLVM_COV_TARGET_DIR")
    try:
        os.environ["CARGO_LLVM_COV_TARGET_DIR"] = "target/definitely-not-here"
        check(
            "an instrumented directory that does not exist is an error",
            raises(instrumented_dir),
            True,
        )
    finally:
        if missing_override is None:
            os.environ.pop("CARGO_LLVM_COV_TARGET_DIR", None)
        else:
            os.environ["CARGO_LLVM_COV_TARGET_DIR"] = missing_override

    # --- the instrumented objects the report is allowed to see ---------------
    original_packages_for_prune = globals()["workspace_packages"]
    prune_packages = [
        {"name": "clio", "deps": set(), "dir": "crates/clio",
         "targets": ["clio", "additive_tools_test"]},
        {"name": "clio-store", "deps": set(), "dir": "crates/clio-store",
         "targets": ["clio-store", "mcp_principal_binding"]},
        {"name": "clio-ops", "deps": set(), "dir": "crates/clio-ops",
         "targets": ["clio-ops", "clio_ops_conformance"]},
    ]
    try:
        globals()["workspace_packages"] = lambda: prune_packages
        with tempfile.TemporaryDirectory(prefix="scoped-runner-objects-") as scratch:
            object_root = Path(scratch)
            (object_root / "debug" / "deps").mkdir(parents=True)
            deps = object_root / "debug" / "deps"
            kept = [
                # The selected packages' own binaries. Cargo spells the file name
                # of `clio-store`'s test binary with an UNDERSCORE, so a rule
                # matching only the dashed package name would delete a binary the
                # run needs.
                deps / "clio-2b3cf5c1458a8f42",
                deps / "clio_store-98ac1e2f3b4d5c6e",
                # A selected package's integration-test targets, which are named
                # after the TARGET and not after the package.
                deps / "mcp_principal_binding-7f6e5d4c3b2a1908",
                deps / "additive_tools_test-1011ac0a5f159ade",
            ]
            deleted = [
                deps / "clio_ops-fa1e5933542c28db",
                deps / "clio_ops_conformance-5150f1e2d3c4b5a6",
            ]
            # `clio` is the workspace root's package and `clio-store` is a
            # different package whose name starts with it. A prefix rule, or one
            # that only matched the dashed spelling, would get this pair wrong.
            for name in kept + deleted:
                name.write_text("x")
                os.chmod(name, 0o755)
            decoy_library = deps / "clio_store-1a2b3c4d5e6f7081.rlib"
            decoy_library.write_text("x")
            decoy_depinfo = deps / "clio_ops-1a2b3c4d5e6f7081.d"
            decoy_depinfo.write_text("x")
            prune_log = io.StringIO()
            original_stdout_for_prune = sys.stdout
            try:
                sys.stdout = prune_log
                deleted_count = prune_foreign_objects(
                    object_root, ["clio", "clio-store"]
                )
            finally:
                sys.stdout = original_stdout_for_prune
            check(
                "every selected package's binary and target binary survives",
                [path.exists() for path in kept],
                [True, True, True, True],
            )
            check(
                "an unselected package's binary is deleted",
                [path.exists() for path in deleted],
                [False, False],
            )
            check("the number deleted is printed", "deleted 2" in prune_log.getvalue(), True)
            check("the number deleted is returned", deleted_count, 2)
            # These two are named exactly like the objects above and are not
            # objects to the merge, so deleting them would cost a rebuild and fix
            # nothing.
            check("a library archive is not an object and survives", decoy_library.exists(), True)
            check("a dependency-info file survives", decoy_depinfo.exists(), True)

        # The pair that a prefix rule gets wrong, on its own: selecting only
        # `clio-store` keeps `clio_store-<hash>` and deletes `clio-<hash>`.
        with tempfile.TemporaryDirectory(prefix="scoped-runner-prefix-") as scratch:
            prefix_root = Path(scratch) / "llvm-cov-target"
            prefix_deps = prefix_root / "debug" / "deps"
            prefix_deps.mkdir(parents=True)
            store_binary = prefix_deps / "clio_store-98ac1e2f3b4d5c6e"
            root_binary = prefix_deps / "clio-2b3cf5c1458a8f42"
            for name in (store_binary, root_binary):
                name.write_text("x")
                os.chmod(name, 0o755)
            original_stdout_for_prefix = sys.stdout
            try:
                sys.stdout = io.StringIO()
                prune_foreign_objects(prefix_root, ["clio-store"])
            finally:
                sys.stdout = original_stdout_for_prefix
            check("selecting a package keeps its own underscored binary", store_binary.exists(), True)
            check(
                "one package's name being a prefix of another's does not keep it",
                root_binary.exists(),
                False,
            )

        # A run that deletes nothing still has to say so, because the number is
        # the evidence that the pruning happened at all.
        with tempfile.TemporaryDirectory(prefix="scoped-runner-all-") as scratch:
            all_root = Path(scratch)
            only = all_root / "clio-2b3cf5c1458a8f42"
            only.write_text("x")
            os.chmod(only, 0o755)
            all_log = io.StringIO()
            original_stdout_for_all = sys.stdout
            try:
                sys.stdout = all_log
                none_deleted = prune_foreign_objects(
                    all_root, ["clio", "clio-store", "clio-ops"]
                )
            finally:
                sys.stdout = original_stdout_for_all
            check(
                "selecting every package deletes nothing",
                (none_deleted, only.exists(), "deleted 0" in all_log.getvalue()),
                (0, True, True),
            )
        # An empty selection is a mass deletion, not an empty deletion. With no
        # names the keep pattern matches only the empty stem, so every object it
        # can see goes. The caller returns early on an empty selection, so this
        # path is unreachable from the command line today; the refusal lives in the
        # function so it does not depend on a caller two frames away.
        with tempfile.TemporaryDirectory(prefix="scoped-runner-empty-prune-") as scratch:
            empty_root = Path(scratch)
            doomed = empty_root / "clio-store-1a2b3c4d"
            doomed.write_bytes(b"\x7fELF fake executable")
            check(
                "an empty selection is refused before anything is deleted",
                raises(prune_foreign_objects, empty_root, []),
                True,
            )
            check("and the refused run deleted nothing", doomed.exists(), True)
    finally:
        globals()["workspace_packages"] = original_packages_for_prune

    # --- the one-package mode, wired end to end without a build --------------
    #
    # Every step is recorded rather than run, because what is under test is the
    # order, the directory, the package, and the report path: four things that a
    # real run would only prove by producing the wrong rows and taking an hour.
    calls: list[tuple] = []
    original_mode_globals = {
        name: globals()[name]
        for name in (
            "workspace_packages",
            "clear_profile_buffers",
            "run_coverage_tests",
            "instrumented_dir",
            "drop_invalid_profraws",
            "prune_foreign_objects",
            "write_coverage_report",
            "load_report",
        )
    }
    one_package_report = {
        "data": [
            {
                "files": [
                    {
                        "filename": str(ROOT / "crates/clio-store/src/sqlite.rs"),
                        "segments": [],
                        "summary": {
                            "lines": {"count": 10, "covered": 9, "percent": 90.0},
                            "functions": {"count": 4, "covered": 3, "percent": 75.0},
                        },
                    },
                    # A file of another package. It cannot happen when the
                    # directory does its job, so if it appears in the report the
                    # directory did not, and a mode whose whole point is a
                    # correct object list has to say so rather than print rows.
                    {
                        "filename": str(ROOT / "crates/clio-ops/src/mod.rs"),
                        "segments": [],
                        "summary": {
                            "lines": {"count": 5, "covered": 1, "percent": 20.0},
                            "functions": {"count": 2, "covered": 1, "percent": 50.0},
                        },
                    },
                ]
            }
        ]
    }
    try:
        globals()["workspace_packages"] = lambda: prune_packages
        globals()["clear_profile_buffers"] = lambda env: calls.append(("clear", env))
        globals()["run_coverage_tests"] = lambda pkgs, filt=None, env=None: calls.append(
            ("test", tuple(pkgs), dict(env or {}))
        )
        # The real one refuses a directory that does not exist, which is the
        # right behaviour and impossible here, so it is replaced by its own
        # resolution: what is under test is WHICH directory the mode picked, and
        # that it picked it from its own run's environment.
        globals()["instrumented_dir"] = lambda env=None: (
            calls.append(("dir", resolved_instrumented_dir(env))) or calls[-1][1]
        )
        globals()["drop_invalid_profraws"] = lambda directory: calls.append(("buffers", Path(directory)))
        globals()["prune_foreign_objects"] = lambda directory, selected: calls.append(
            ("prune", Path(directory), tuple(selected))
        ) or 0
        globals()["write_coverage_report"] = lambda path, env=None: calls.append(("report", Path(path)))
        globals()["load_report"] = lambda path: one_package_report
        original_stdout_for_mode = sys.stdout
        original_stderr_for_mode = sys.stderr
        mode_out, mode_err = io.StringIO(), io.StringIO()
        try:
            sys.stdout = mode_out
            sys.stderr = mode_err
            package_code = main(["--coverage-package", "clio-store"])
        finally:
            sys.stdout = original_stdout_for_mode
            sys.stderr = original_stderr_for_mode
        check("the one-package mode reports and does not judge", package_code, EXIT_OK)
        check(
            "it runs the same steps in the same order",
            [call[0] for call in calls],
            ["clear", "test", "dir", "buffers", "prune", "report"],
        )
        check("its test step names exactly this package", calls[1][1], ("clio-store",))
        expected_directory = (ROOT / package_target_override("clio-store")).resolve()
        check(
            "it resolves the package's own instrumented directory",
            calls[2][1],
            expected_directory,
        )
        check(
            "its hygiene steps run against that directory, not the shared one",
            (calls[3][1], calls[4][1]),
            (expected_directory, expected_directory),
        )
        check(
            "its report path is the package's own, not the gate's",
            calls[5][1],
            package_report_path("clio-store"),
        )
        check(
            "its tools build into that directory, not the shared one",
            calls[1][2].get(CARGO_LLVM_COV_TARGET_KEY),
            package_target_override("clio-store"),
        )
        check(
            "it prunes the object list rather than trusting the directory",
            calls[4][2],
            ("clio-store",),
        )
        check(
            "it prints a row for the package's own file",
            "crates/clio-store/src/sqlite.rs lines 90.00% functions 75.00%" in mode_out.getvalue(),
            True,
        )
        check(
            "a file of another package is a warning, not a silent row",
            "did not isolate it" in mode_err.getvalue(),
            True,
        )
        check(
            "it says it compared no floor",
            "no verdict is reported" in mode_out.getvalue(),
            True,
        )
    finally:
        for name, value in original_mode_globals.items():
            globals()[name] = value

    # The same mode on a red suite. The run step records its package and then
    # fails the way a failing test does, and every later step still has to
    # happen: the report is the output, so it is written from the buffers that
    # run left, and the exit code carries the run's verdict rather than taking
    # the report away.
    red_calls: list[tuple] = []
    original_red_globals = {
        name: globals()[name]
        for name in (
            "workspace_packages",
            "clear_profile_buffers",
            "run_coverage_tests",
            "instrumented_dir",
            "drop_invalid_profraws",
            "prune_foreign_objects",
            "write_coverage_report",
            "load_report",
        )
    }

    def _failing_test_step(pkgs, filt=None, env=None) -> None:
        red_calls.append(("test", tuple(pkgs)))
        raise RunError("the instrumented test run failed (exit 100)")

    try:
        globals()["workspace_packages"] = lambda: prune_packages
        globals()["clear_profile_buffers"] = lambda env: red_calls.append(("clear", env))
        globals()["run_coverage_tests"] = _failing_test_step
        globals()["instrumented_dir"] = lambda env=None: (
            red_calls.append(("dir", resolved_instrumented_dir(env))) or red_calls[-1][1]
        )
        globals()["drop_invalid_profraws"] = lambda directory: red_calls.append(
            ("buffers", Path(directory))
        )
        globals()["prune_foreign_objects"] = lambda directory, selected: red_calls.append(
            ("prune", Path(directory), tuple(selected))
        ) or 0
        globals()["write_coverage_report"] = lambda path, env=None: red_calls.append(
            ("report", Path(path))
        )
        globals()["load_report"] = lambda path: one_package_report
        original_stdout_for_red = sys.stdout
        original_stderr_for_red = sys.stderr
        red_out, red_err = io.StringIO(), io.StringIO()
        try:
            sys.stdout = red_out
            sys.stderr = red_err
            red_code = main(["--coverage-package", "clio-store"])
        finally:
            sys.stdout = original_stdout_for_red
            sys.stderr = original_stderr_for_red
        check(
            "a red suite runs the same steps in the same order",
            [call[0] for call in red_calls],
            ["clear", "test", "dir", "buffers", "prune", "report"],
        )
        check(
            "a red suite still writes the package's own report",
            red_calls[-1][1],
            package_report_path("clio-store"),
        )
        check(
            "the failed run is named",
            "the instrumented test run failed" in red_err.getvalue(),
            True,
        )
        check(
            "the report is printed even though the run failed",
            "crates/clio-store/src/sqlite.rs lines 90.00% functions 75.00%"
            in red_out.getvalue(),
            True,
        )
        check("a red suite ends with the run's non-zero status", red_code, EXIT_FAIL)
    finally:
        for name, value in original_red_globals.items():
            globals()[name] = value

    # A name that is not a package is a usage error and must build nothing: an
    # empty measurement that prints a header and exits 0 is the failure this
    # avoids.
    unknown_log = io.StringIO()
    original_stderr_for_unknown = sys.stderr
    original_packages_for_unknown = globals()["workspace_packages"]
    try:
        globals()["workspace_packages"] = lambda: prune_packages
        sys.stderr = unknown_log
        check(
            "an unknown package is a usage error",
            main(["--coverage-package", "clio-nonexistent"]),
            EXIT_USAGE,
        )
    finally:
        sys.stderr = original_stderr_for_unknown
        globals()["workspace_packages"] = original_packages_for_unknown
    check("the unknown package is named", "clio-nonexistent" in unknown_log.getvalue(), True)

    # A measurement that could not be produced is exit 3. Exit 0 would say the
    # package measured clean, which is the opposite of what happened.
    def _raising_package_run(name: str) -> Path:
        raise RunError("the instrumented build directory does not exist")

    original_run_for_package_error = globals()["run_package_coverage"]
    error_log = io.StringIO()
    original_stderr_for_package_error = sys.stderr
    try:
        globals()["workspace_packages"] = lambda: prune_packages
        globals()["run_package_coverage"] = _raising_package_run
        sys.stderr = error_log
        check(
            "a measurement that could not be produced is a run error",
            main(["--coverage-package", "clio-store"]),
            EXIT_RUN_ERROR,
        )
    finally:
        sys.stderr = original_stderr_for_package_error
        globals()["run_package_coverage"] = original_run_for_package_error
        globals()["workspace_packages"] = original_packages_for_unknown

    # A deletion is only ever allowed inside the instrumented directory. The
    # check exists because a mistake in resolving that directory would otherwise
    # reach an ordinary build directory.
    with tempfile.TemporaryDirectory(prefix="scoped-runner-boundary-") as scratch:
        boundary_root = Path(scratch)
        inside_dir = boundary_root / "llvm-cov-target"
        inside_dir.mkdir()
        escapee = boundary_root / "outside.profraw"
        escapee.write_bytes(b"")
        check(
            "a path outside the instrumented directory is not inside it",
            _inside(escapee, inside_dir),
            False,
        )
        check(
            "a path inside the instrumented directory is inside it",
            _inside(inside_dir / "a.profraw", inside_dir),
            True,
        )

    # The header size is a measured constant and not a plausible-looking number,
    # so its own value is asserted. If it changes, the assertion is what notices.
    check("the raw profile header size is measured", RAW_PROFILE_HEADER_BYTES, 128)

    # A pure rename adds no line because nothing about the file changed, which is a
    # different fact from this change never having touched it. It is a diagnostic
    # and changes no verdict, so these cases pin the parse and nothing else.
    original_base_for_none = globals()["merge_base"]
    original_git_for_renames = globals()["git"]
    try:
        globals()["merge_base"] = lambda base: None
        check("a change with no merge base reports no renames", renames("master"), set())
        globals()["merge_base"] = lambda base: "abc123"
        globals()["git"] = lambda *a: "R100\0crates/a/src/old.rs\0crates/a/src/new.rs\0"
        check("a pure rename names its new path", renames("master"), {"crates/a/src/new.rs"})
        globals()["git"] = lambda *a: "M\0crates/a/src/edited.rs\0"
        check("an edit is not a rename", renames("master"), set())
    finally:
        globals()["merge_base"] = original_base_for_none
        globals()["git"] = original_git_for_renames

    # The gate itself, in both directions, at the one number. These are the cases
    # that hold the single rule in place, and they are deliberately about the
    # percentage alone: the counts a report also carries are read and printed, and
    # nothing compares them, so a row whose covered count disagrees with its own
    # percentage cannot change a verdict.
    counted = judge(
        {
            # 3 of 191 lines covered is 1.57%, and the row says 90%. The
            # percentage is what the rule reads, so this fails on the percentage
            # and the disagreement in the counts is invisible to the verdict.
            "crates/a/src/one.rs": CoverageRow(90.0, 100.0, 191, 24, 3, 24),
        },
        ["crates/a/src/one.rs"],
    )[0]
    check("the line percentage is the only input", len(counted), 0)
    below = judge(
        {"crates/a/src/two.rs": CoverageRow(79.99, 0.0)}, ["crates/a/src/two.rs"]
    )[0]
    check("a file under the floor fails", len(below), 1)
    check(
        "the message quotes the one number and nothing else",
        below[0],
        "crates/a/src/two.rs lines 79.99% (floor 80.00)",
    )
    check(
        "function coverage never appears in a failure",
        any("function" in line for line in below),
        False,
    )
    check(
        "a file at 0% functions passes when its lines clear the floor",
        judge({"crates/a/src/three.rs": CoverageRow(100.0, 0.0)}, ["crates/a/src/three.rs"])[0],
        [],
    )
    check(
        "a file at 0% lines fails even at 100% functions",
        len(judge({"crates/a/src/four.rs": CoverageRow(0.0, 100.0)}, ["crates/a/src/four.rs"])[0]),
        1,
    )
    mixed_offenders, mixed_missing = judge(
        {
            "crates/a/src/good.rs": CoverageRow(95.0, 100.0),
            "crates/a/src/thin.rs": CoverageRow(70.0, 100.0),
        },
        ["crates/a/src/good.rs", "crates/a/src/thin.rs", "crates/a/src/absent.rs"],
    )
    check("only the file below the floor is an offender", len(mixed_offenders), 1)
    check("the offender is the thin one", "thin.rs" in mixed_offenders[0], True)
    check(
        "the file the report lacks is named, not judged",
        mixed_missing,
        ["crates/a/src/absent.rs"],
    )
    check(
        "a caller cannot pass a second, competing number",
        judge({"crates/a/src/five.rs": CoverageRow(79.0, 0.0)}, ["crates/a/src/five.rs"], 70.0)[0],
        [],
    )


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


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
