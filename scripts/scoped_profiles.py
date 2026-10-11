"""The instrumented build directory and the raw profile buffers and objects in it."""

from __future__ import annotations

import json
import os
import re
import stat
import subprocess
from pathlib import Path, PurePosixPath
from typing import Any, Collection, Iterable, NamedTuple, Sequence

from scoped_core import ROOT, RunError
from scoped_select import workspace_packages

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
