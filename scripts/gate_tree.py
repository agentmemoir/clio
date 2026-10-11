from __future__ import annotations

import hashlib
import os
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

from gate_kinds import ENV_KEYS, GATE_SCOPE_DEFAULT, GATE_SCOPE_OFF, KEY_SCHEME

class GateError(Exception):
    """The gate cannot operate; the caller should run the command directly."""


@dataclass(frozen=True)
class TreeState:
    head: str
    contents: tuple[tuple[str, str], ...]
    changed: tuple[str, ...]


@dataclass(frozen=True)
class Member:
    name: str
    root: str
    deps: tuple[str, ...]


def _git(cwd: str, *args: str) -> bytes | None:
    try:
        proc = subprocess.run(["git", *args], cwd=cwd, capture_output=True)
    except OSError:
        return None
    return proc.stdout if proc.returncode == 0 else None


def _splitz(data: bytes) -> list[str]:
    return [chunk.decode("utf-8", "surrogateescape") for chunk in data.split(b"\0") if chunk]


def repo_root(cwd: str) -> str | None:
    out = _git(cwd, "rev-parse", "--show-toplevel")
    return out.decode().strip() if out else None


def read_tree_state(root: str) -> TreeState:
    """HEAD plus the content and git-visible mode of every path that differs.

    The key describes the working tree, not the index, so staging a change does
    not move it. A path's bytes reach the key the same way whether it is tracked
    or untracked: hashing `git diff HEAD` and the untracked list separately put a
    new file in the key once as a hashed untracked entry and then, after `git
    add`, inside the diff text, so the same bytes keyed two entries. The mode is
    hashed beside the bytes because `git diff HEAD` used to carry a mode change
    in its text, and a content-only key would silently lose that sensitivity.
    """
    head = _git(root, "rev-parse", "HEAD")
    names = _git(root, "diff", "HEAD", "--name-only", "-z")
    untracked = _git(root, "ls-files", "--others", "--exclude-standard", "-z")
    if head is None or names is None or untracked is None:
        raise GateError("git state unavailable")
    paths = sorted(set(_splitz(names)) | set(_splitz(untracked)))
    entries: list[tuple[str, str]] = []
    for rel in paths:
        path = Path(root) / rel
        try:
            info = path.stat()
        except FileNotFoundError:
            # A deleted path has no bytes; the name and this marker still
            # separate a deletion from the same path restored.
            entries.append((rel, "deleted"))
            continue
        except OSError as e:
            raise GateError(f"cannot stat changed file {rel}: {e}") from e
        try:
            body = path.read_bytes()
        except OSError as e:
            raise GateError(f"cannot read changed file {rel}: {e}") from e
        # Git tracks only the executable bit, so a mode-only change on a file
        # whose content is already in the key must still move the key.
        mode = "x" if info.st_mode & 0o111 else "-"
        entries.append((rel, f"{mode}:{hashlib.sha256(body).hexdigest()}"))
    return TreeState(
        head=head.decode().strip(),
        contents=tuple(entries),
        changed=tuple(paths),
    )


def _effective_env(env: dict[str, str], key: str) -> str:
    """The value a command actually ran under, for the variables that have one.

    Most keys record themselves. A switch whose only branch is "off" records the
    side of the switch it was on, so a value the code never acts on does not
    spend a cache entry, and so leaving the switch out and naming its default
    stay one entry instead of quietly becoming two.
    """
    if key == "GATE_SCOPE":
        return GATE_SCOPE_OFF if env.get("GATE_SCOPE", GATE_SCOPE_DEFAULT) == GATE_SCOPE_OFF else GATE_SCOPE_DEFAULT
    return env.get(key, "")


def fingerprint(state: TreeState, env: dict[str, str], kind: str, argv: list[str]) -> str:
    """The cache key: what makes one stored verdict distinguishable from another.

    The kind is hashed as well as the argument vector. The vector alone already
    separates the commands seen so far, but the kind is the narrower promise:
    it is what names the log the verdict is published under and which artifacts
    are saved and restored, so two commands that hash to one entry must also
    agree about that. Hashing it makes "a stored verdict is replayed only for
    the command it was measured from" a property of the key rather than of how
    carefully the argument vector happens to be spelled.

    Cargo and make are keyed by command name; make's `BASE` assignment is keyed as an environment value, so both spellings share one entry.
    """
    _scheme = getattr(sys.modules.get("gate"), "KEY_SCHEME", KEY_SCHEME)
    executable = Path(argv[0]).name
    h = hashlib.sha256()
    h.update(_scheme)
    h.update(state.head.encode() + b"\0")
    for rel, digest in state.contents:
        h.update(f"F\0{rel}\0{digest}\0".encode())
    for key in ENV_KEYS:
        value = _effective_env(env, key)
        if key == "BASE" and executable == "make":
            assignments = [arg.partition("=")[2] for arg in argv[1:] if arg.startswith("BASE=")]
            if assignments:
                value = assignments[-1]
        elif key == "BASE":
            value = ""
        h.update(f"E\0{key}\0{value}\0".encode())
    h.update(f"K\0{kind}\0".encode())
    args = argv[1:]
    if executable == "make":
        args = [arg for arg in args if not arg.startswith("BASE=")]
    normalized_argv = [executable if executable in {"cargo", "make"} else argv[0], *args]
    h.update(b"C\0" + b"\0".join(arg.encode() for arg in normalized_argv) + b"\0")
    return h.hexdigest()


def cache_root() -> Path:
    base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "clio-gate"


def repo_key(root: str) -> str:
    return hashlib.sha256(str(Path(root).resolve()).encode()).hexdigest()[:16]


def entry_dir(root: str, fp: str) -> Path:
    return cache_root() / repo_key(root) / fp[:16]
