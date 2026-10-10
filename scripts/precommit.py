#!/usr/bin/env python3
"""Run every registered pre-commit hook and fail if any of them fails.

A hook is an executable file. The runner finds hooks, runs each one with the
list of tracked text files as arguments, and exits non-zero if any hook exited
non-zero. The runner has no built-in checks of its own and knows nothing about
what any hook looks for, so a check can live outside this repository without
this file mentioning where it lives or what it checks.

Where hooks come from
---------------------
Three sources, merged and de-duplicated by resolved path:

1. ``hooks.d/`` at the repository root. Tracked, so shared with everyone who
   clones, and the right home for a check the whole project should run.
2. ``.clio-hooks/`` at the repository root. Listed in ``.gitignore``, so it is
   machine-local: a hook dropped here is never staged, never pushed, and never
   appears in a diff. This is how a check kept outside the repository joins the
   run without the repository disclosing that it exists.
3. Every path in ``CLIO_PRECOMMIT_HOOKS``, split on the platform separator.
   A directory contributes its executable children; a file is taken as-is.

Two scopes
----------
A hook is either scoped to the commit or not, and the hook says which by
carrying a line in its own file:

    # precommit-scope: manual

``manual`` means the check runs when somebody asks for it and not on every
commit. Everything else is commit-scoped, including a hook that says nothing:
the default is the scope that cannot be lost, so a new check runs on commit
until its author takes it out. A value this runner does not recognise is an
error rather than a guess, because an author who wrote ``manul`` meant one
scope and would get the other.

The distinction exists because a commit hook cannot afford to be slow. A check
that reads the staged files is bounded by the size of a commit; a check that
runs a test suite is not, and charging every commit for it is how a gate gets
skipped. The deliberate cost of the split is that a manual check is not run by
committing, only by asking, so it needs to be named somewhere a reader looks.

Reporting
---------
The run lists every hook it found, so a check that is registered but never
executed is visible. A hook directory that exists but is empty is reported
rather than passed over in silence, because a gate that quietly stops running
looks exactly like a gate that is passing. A commit run says which hooks it
left out, for the same reason.

Usage:
    python3 scripts/precommit.py                 # every hook, every tracked file
    python3 scripts/precommit.py --staged        # only the staged files
    python3 scripts/precommit.py --commit        # commit-scoped hooks, staged files
    python3 scripts/precommit.py --commit --all-files   # commit-scoped hooks, every tracked file
    python3 scripts/precommit.py --list          # names only, run nothing
    python3 scripts/precommit.py --skip phase-timer
    python3 scripts/precommit.py --only no-spec-ids

Exit codes:
    0   every hook ran and every hook passed
    1   at least one hook failed
    2   usage error
    3   the hook set could not be established (not a repository, git missing)

Environment (set for each hook):
    CLIO_PRECOMMIT_ROOT   absolute path of the repository root
    CLIO_PRECOMMIT_MODE   ``staged`` or ``all``
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from precommit_hooks import SetupError, discover, partition_by_scope, select  # noqa: E402

EXIT_FAIL = 1
EXIT_USAGE = 2
EXIT_SETUP = 3

#: Tracked and ignored directories whose children are never hooks.
SKIP_DIRS = {".git", "target", "node_modules", "__pycache__"}


def repo_root() -> Path:
    """The repository root, or raise if git cannot say."""
    try:
        done = subprocess.run(  # noqa: S603
            ["git", "rev-parse", "--show-toplevel"],  # noqa: S607
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as error:
        raise SetupError(f"git is not runnable: {error}") from error
    if done.returncode != 0:
        raise SetupError("not inside a git repository")
    return Path(done.stdout.strip()).resolve()


def git_lines(root: Path, *args: str) -> list[str]:
    """Run a git command and return its stdout split on NUL.

    Bytes, not text: a NUL-separated path list is decoded by path encoding
    rules rather than by the locale, so a file whose name is not valid in the
    ambient encoding still comes back as a name and not as a decoding error.
    """
    try:
        done = subprocess.run(  # noqa: S603
            ["git", *args],  # noqa: S607
            capture_output=True,
            cwd=root,
            check=False,
        )
    except OSError as error:
        raise SetupError(f"git is not runnable: {error}") from error
    if done.returncode != 0:
        raise SetupError(f"git {' '.join(args)} failed")
    return [
        name.decode("utf-8", "surrogateescape")
        for name in done.stdout.split(b"\0")
        if name
    ]


def subject_files(root: Path, staged: bool) -> list[str]:
    """Repository-relative paths of the files this run is about.

    Tracked files rather than every file on disk, so a build artifact or an
    ignored checkout is never scanned. Text only, decided by decoding, because
    a rule that covers some extensions quietly misses every other one.
    """
    raw = git_lines(root, "ls-files", "-z", "--cached") if not staged else git_lines(
        root, "diff", "--cached", "--name-only", "-z"
    )
    keep: list[str] = []
    for name in raw:
        path = root / name
        if any(part in SKIP_DIRS for part in Path(name).parts):
            continue
        try:
            path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        keep.append(name)
    return sorted(keep)


def run_hook(hook: Path, files: list[str], root: Path, staged: bool) -> tuple[int, str]:
    """Run one hook. Returns (exit code, combined output)."""
    environment = dict(os.environ)
    environment["CLIO_PRECOMMIT_ROOT"] = str(root)
    environment["CLIO_PRECOMMIT_MODE"] = "staged" if staged else "all"
    if os.access(hook, os.X_OK):
        argv = [str(hook)]
    else:
        argv = [sys.executable, str(hook)]
    done = subprocess.run(  # noqa: S603
        argv + files,
        capture_output=True,
        text=True,
        env=environment,
        cwd=root,
        check=False,
    )
    return done.returncode, (done.stdout + done.stderr).rstrip()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Run every registered pre-commit hook.",
    )
    parser.add_argument("--staged", action="store_true", help="check only staged files")
    parser.add_argument(
        "--commit",
        action="store_true",
        help="run only the commit-scoped hooks, over the staged files",
    )
    parser.add_argument(
        "--all-files",
        action="store_true",
        help="with --commit, check every tracked file instead of the staged ones",
    )
    parser.add_argument("--list", action="store_true", help="list hooks and exit")
    parser.add_argument(
        "--only", action="append", default=[], metavar="NAME", help="run only these hooks"
    )
    parser.add_argument(
        "--skip", action="append", default=[], metavar="NAME", help="do not run these hooks"
    )
    args = parser.parse_args(argv)
    if args.all_files and (args.staged or not args.commit):
        parser.error("--all-files goes with --commit, and not with --staged")

    # A commit hook reads what is being committed and nothing else, so --commit
    # implies --staged. Staging a file is what puts it in front of a check.
    # --all-files is the one way to widen that: it is how a person asks the commit
    # checks to read the whole tree, which is what `make precommit` does.
    staged = args.staged or (args.commit and not args.all_files)

    try:
        root = repo_root()
        found, notes, broken = discover(root)
        commit_scoped, manual = partition_by_scope(found)
        chosen = select(commit_scoped if args.commit else found, args.only, args.skip)
    except SetupError as error:
        print(f"precommit: cannot run hooks: {error}", file=sys.stderr)
        return EXIT_SETUP

    for note in notes:
        print(f"precommit: note: {note}", file=sys.stderr)
    for item in broken:
        print(f"precommit: broken hook: {item}", file=sys.stderr)
    if broken:
        print(
            f"precommit: {len(broken)} registered hook(s) cannot run. Re-register "
            f"them, or delete the link if the check is no longer wanted. Not "
            f"reporting a verdict, because the hooks that were asked for did not run.",
            file=sys.stderr,
        )
        return EXIT_FAIL

    if args.commit and manual:
        # Said out loud, because a check that is registered and never runs is
        # the one state this runner exists to make visible.
        print(
            "precommit: not commit-scoped, so not run here: "
            + ", ".join(hook.stem for hook in manual)
        )

    if args.list:
        for hook in found:
            marker = " (selected)" if hook in chosen else " (skipped)"
            print(f"{hook.stem}{marker}")
        print(f"\n{len(chosen)} of {len(found)} hook(s) would run.")
        return EXIT_FAIL if found and not chosen else 0

    if not chosen:
        if args.commit:
            # Not a failure. There is no commit-scoped check to run on this
            # machine, or the one that exists was asked out of this run, so
            # there is no gate to enforce, and refusing every commit would only
            # teach the person committing to pass --no-verify. A broken
            # registration still failed above, before this point.
            if commit_scoped:
                print(
                    "precommit: every commit-scoped hook was left out of this run "
                    f"({', '.join(hook.stem for hook in commit_scoped)}), so nothing "
                    "was gated.",
                    file=sys.stderr,
                )
            else:
                print(
                    "precommit: no commit-scoped hook is registered, so the commit "
                    "check had nothing to run. The checks that are not commit-scoped "
                    "run when they are asked for by name.",
                    file=sys.stderr,
                )
            return 0
        print("precommit: no hook is registered, so nothing was checked.", file=sys.stderr)
        return EXIT_FAIL

    files = subject_files(root, staged)
    scope = "staged file(s)" if staged else "tracked file(s)"
    if not files:
        # Said out loud, because a green run over an empty list is
        # indistinguishable in its output from a green run over the tree.
        print(
            f"precommit: no {scope} to check, so every hook below had nothing to "
            f"read. Stage something, or drop --staged to check the tracked tree.",
            file=sys.stderr,
        )
    print(
        f"precommit: {len(chosen)} hook(s) over {len(files)} {scope}:\n"
        + "\n".join(f"  {hook.stem}" for hook in chosen)
    )

    failures: list[tuple[str, str]] = []
    for hook in chosen:
        code, output = run_hook(hook, files, root, staged)
        if code == 0:
            print(f"\n== {hook.stem}: pass")
            if output:
                print(output)
            continue
        failures.append((hook.stem, output))
        print(f"\n== {hook.stem}: FAIL (exit {code})", file=sys.stderr)
        if output:
            print(output, file=sys.stderr)

    print()
    if failures:
        names = ", ".join(name for name, _ in failures)
        print(f"precommit: {len(failures)} hook(s) failed: {names}", file=sys.stderr)
        return EXIT_FAIL
    print(f"precommit: all {len(chosen)} hook(s) passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
