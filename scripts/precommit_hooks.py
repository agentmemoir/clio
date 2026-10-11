"""Find the registered pre-commit hooks, decide which scope each belongs to, and pick some.

This is the discovery half of `scripts/precommit.py`, which documents where hooks
come from and what the two scopes mean. Nothing here runs a hook or reads the
files a hook is given.
"""

from __future__ import annotations

import os
import re
import stat
import subprocess
from pathlib import Path

#: Tracked directories searched for hooks, in the order they run.
TRACKED_HOOK_DIR = "hooks.d"

#: Machine-local directories searched for hooks. Never tracked.
LOCAL_HOOK_DIRS = (".clio-hooks",)

#: Environment variable naming extra hook files or directories.
HOOK_PATH_ENV = "CLIO_PRECOMMIT_HOOKS"

#: A hook file is one of these, with or without an executable bit.
HOOK_SUFFIXES = {".py", ".sh"}

#: The line a hook carries to say which scope it belongs to. Matched in a
#: comment so the declaration is part of the hook rather than a second file
#: beside it, and so a hook copied anywhere still declares its own scope.
#: The `\r?` is there because a checkout with Windows line endings would
#: otherwise match nothing and silently keep the default scope, which for a
#: commit-scoped check means a gate that stops running on those machines.
SCOPE_DECLARATION = re.compile(
    r"^[ \t]*#[ \t]*precommit-scope[ \t]*:[ \t]*(?P<scope>[A-Za-z-]+)[ \t]*\r?$",
    re.MULTILINE,
)

#: Scope values this runner understands. Anything else is a SetupError.
SCOPE_MANUAL = "manual"
SCOPE_COMMIT = "commit"
KNOWN_SCOPES = (SCOPE_COMMIT, SCOPE_MANUAL)


class SetupError(Exception):
    """The hook set could not be established, so no verdict is possible."""



def is_hook_file(path: Path) -> bool:
    """A hook is a known-suffix script, or any file carrying the execute bit.

    The suffix decides on its own and the execute bit is not consulted for it,
    because the runner knows how to start a non-executable script through the
    interpreter. Requiring the bit would mean a hook whose mode was lost in a
    checkout, an archive, or a `cp` was skipped in silence, and a gate that
    stops running is indistinguishable from a gate that is passing. Only a file
    with no recognised suffix has to prove itself by being executable.
    """
    if path.is_symlink() and not path.exists():
        return False  # a dangling link is a moved checkout, not a hook
    if not path.is_file():
        return False
    if path.suffix in HOOK_SUFFIXES:
        return True
    try:
        return bool(path.stat().st_mode & stat.S_IXUSR)
    except OSError:
        return False


def hooks_in(directory: Path) -> tuple[list[Path], list[Path]]:
    """Return (hooks, dangling links) directly inside `directory`.

    A symlink whose target is gone is reported rather than ignored: it means a
    checkout moved while a hook stayed registered, and the check it stood for is
    no longer running.
    """
    if not directory.is_dir():
        return [], []
    children = sorted(directory.iterdir())
    dangling = [child for child in children if child.is_symlink() and not child.exists()]
    hooks = [child for child in children if is_hook_file(child)]
    return hooks, dangling


def declared_scope(hook: Path) -> str:
    """The scope a hook declares for itself, or the commit scope by default.

    A hook that says nothing is commit-scoped. That default is the whole point:
    a check nobody has classified yet runs on commit, so the worst case is a
    slow commit rather than a check that quietly stopped running.
    """
    try:
        text = hook.read_text(encoding="utf-8", errors="replace")
    except OSError as error:
        raise SetupError(f"{hook} cannot be read: {error}") from error
    match = SCOPE_DECLARATION.search(text)
    if match is None:
        return SCOPE_COMMIT
    scope = match.group("scope")
    if scope not in KNOWN_SCOPES:
        raise SetupError(
            f"{hook} declares precommit-scope: {scope}, which is not a scope. "
            f"Use one of: {', '.join(KNOWN_SCOPES)}."
        )
    return scope


def partition_by_scope(hooks: list[Path]) -> tuple[list[Path], list[Path]]:
    """Split discovered hooks into (commit-scoped, manual), each name-ordered.

    One read per hook, not one per comparison: a hook is a file on disk that a
    person may be editing, and reading it twice invites the two reads to
    disagree.
    """
    commit_scoped: list[Path] = []
    manual: list[Path] = []
    for hook in hooks:
        bucket = commit_scoped if declared_scope(hook) == SCOPE_COMMIT else manual
        bucket.append(hook)
    return commit_scoped, manual


def env_hook_paths() -> list[Path]:
    """Extra hook locations named by the environment."""
    raw = os.environ.get(HOOK_PATH_ENV, "")
    return [Path(part).expanduser() for part in raw.split(os.pathsep) if part.strip()]


def main_checkout(root: Path) -> Path | None:
    """The top of the main checkout when `root` is a linked worktree, else None.

    The machine-local hook directory is git-ignored and registered in the main
    checkout only, so a worktree has none of its own. git shares one common
    directory across all worktrees of a repository, and the main checkout is the
    folder that holds it.
    """
    try:
        done = subprocess.run(  # noqa: S603
            ["git", "-C", str(root), "rev-parse", "--git-common-dir"],  # noqa: S607
            capture_output=True,
            text=True,
            check=False,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    answer = done.stdout.strip()
    if done.returncode != 0 or not answer:
        return None
    main = (root / answer).resolve().parent  # an absolute answer replaces `root`
    return main if main != root.resolve() else None


def discover(root: Path) -> tuple[list[Path], list[str], list[str]]:
    """Return (hooks, notes, broken).

    Three outcomes, because they mean different things:

    * a hook that runs
    * a directory named as a hook source holding nothing, which is a note: you
      may simply have no machine-local hooks
    * a hook link whose target is gone, which is broken and fails the run. A
      hook was registered here and is no longer running, usually because the
      checkout it lived in moved. Passing that would be the one failure this
      tool exists to prevent: a gate that stopped running and reported success.
    """
    hooks: list[Path] = []
    notes: list[str] = []
    broken: list[str] = []
    seen: set[Path] = set()

    def add(candidates: list[Path]) -> None:
        for candidate in candidates:
            resolved = candidate.resolve()
            if resolved not in seen:
                seen.add(resolved)
                hooks.append(resolved)

    def take(source: Path, label: str) -> None:
        if not source.is_dir():
            return
        found, dangling = hooks_in(source)
        for link in dangling:
            broken.append(f"{label}/{link.name} is registered but its target is gone")
        add(found)
        if not found and not dangling:
            notes.append(f"{label} holds no hook")

    take(root / TRACKED_HOOK_DIR, TRACKED_HOOK_DIR)
    main = main_checkout(root)
    for name in LOCAL_HOOK_DIRS:
        take(root / name, name)
        if main is not None:
            take(main / name, f"{name} of the main checkout")

    for entry in env_hook_paths():
        if entry.is_dir():
            found, dangling = hooks_in(entry)
            for link in dangling:
                broken.append(f"{link} is registered but its target is gone")
            add(found)
            if not found and not dangling:
                notes.append(f"{entry} holds no hook")
        elif entry.is_file():
            add([entry])
        else:
            raise SetupError(f"{HOOK_PATH_ENV} names a path that is not there: {entry}")

    # Sorted across every source, not per source. A number prefix that only
    # orders hooks within one directory would be a lie: a hook registered in the
    # machine-local directory would always run last, whatever it is called, and a
    # reader would expect `20-` to mean `20-`.
    hooks.sort(key=lambda hook: (hook.name, str(hook)))
    return hooks, notes, broken


def select(hooks: list[Path], only: list[str], skip: list[str]) -> list[Path]:
    """Apply --only and --skip to the discovered hooks."""
    chosen = hooks
    if only:
        wanted = set(only)
        chosen = [h for h in chosen if h.stem in wanted or h.name in wanted]
        missing = wanted - {h.stem for h in chosen} - {h.name for h in chosen}
        if missing:
            raise SetupError(f"no hook named: {', '.join(sorted(missing))}")
    if skip:
        unwanted = set(skip)
        chosen = [h for h in chosen if h.stem not in unwanted and h.name not in unwanted]
    return chosen
