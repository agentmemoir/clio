"""Selecting what to measure: the changed files, the packages that own them, and the closure that depends on them."""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path, PurePosixPath
from typing import Any, Collection, Iterable, NamedTuple, Sequence

from scoped_core import ROOT, RunError, offenders

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


def changed_paths(base: str) -> list[str]:
    """Repository-relative paths of every changed file, of any type.

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
    return sorted(paths)


def changed_files(base: str) -> list[str]:
    """Repository-relative paths of changed Rust source files."""
    return [path for path in changed_paths(base) if path.endswith(".rs")]


# Files that are not Rust source but decide what a crate compiles or what its tests read.
# The lock file, the manifests at the root, the toolchain pin and the cargo and nextest
# configuration can change any crate, so they reach every package.
WORKSPACE_WIDE = ("Cargo.lock", "Cargo.toml", "rust-toolchain.toml", ".config/nextest.toml")
WORKSPACE_WIDE_DIRS = (".cargo/",)


def embedders(rel: str, packages: Sequence[dict[str, Any]]) -> set[str]:
    """Packages whose Rust source names `rel`, which they embed with `include_str!`."""
    try:
        found = git("grep", "-l", "-F", rel, "--", "*.rs").splitlines()
    except RunError:  # git grep exits 1 when nothing matches
        return set()
    return {owner for path in found for owner in package_owners(path, packages)}


def build_inputs(paths: Iterable[str], packages: Sequence[dict[str, Any]]) -> tuple[set[str] | None, list[str]]:
    """(the packages the changed non-Rust files reach, or None for all of them, and those files).

    `changed_files` keeps only `.rs` paths, so a change to `Cargo.lock`, a manifest, or a `sql/`
    file a crate embeds selected no package and the gate called the tree clean (issue #69). A file
    inside a package folder reaches that package. A `sql/` file reaches the packages that embed
    it. The files in `WORKSPACE_WIDE`, and a `sql/` file nothing embeds, reach every package,
    because the owner is unclear. Markdown, and files elsewhere (scripts, workflows), reach none.
    """
    reached: set[str] | None = set()
    files: list[str] = []
    for rel in paths:
        if rel.endswith((".rs", ".md")):
            continue
        owners = set(package_owners(rel, packages))
        wide = False
        if not owners and rel.startswith("sql/"):
            owners = embedders(rel, packages)
            wide = not owners
        elif not owners:
            wide = rel in WORKSPACE_WIDE or rel.startswith(WORKSPACE_WIDE_DIRS)
        if not owners and not wide:
            continue
        files.append(rel)
        if wide:
            reached = None
        elif reached is not None:
            reached |= owners
    return reached, files


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
    paths = changed_paths(base)
    files = [path for path in paths if path.endswith(".rs")]
    # A file the change deleted joins the other two exclusions, because it is the
    # same case: there is nothing left in it for a report to hold. Left in the
    # judged list, it was reported as a file the run failed to measure, so every
    # scope failed a change that only removed a file.
    codeless = parse_codeless(files)
    judgeable = [f for f in files if is_judgeable(f, codeless) and is_present(f)]
    packages = workspace_packages()
    if everything:
        return [p["name"] for p in packages], judgeable, files
    reached, build_files = build_inputs(paths, packages)
    if reached is None:
        return [p["name"] for p in packages], judgeable, files
    if not files and not build_files:
        # A genuinely clean tree: no changed file of any type that can move a build.
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
    owners |= reached
    if not owners:
        # Only non-Rust files changed: docs, a Makefile, or a Python script.
        # Nothing there can move a Rust line count.
        return [], judgeable, files
    return package_closure(owners, packages), judgeable, files
