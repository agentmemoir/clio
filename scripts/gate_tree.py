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
    diff_digest: str
    untracked: tuple[tuple[str, str], ...]
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
    head = _git(root, "rev-parse", "HEAD")
    diff = _git(root, "diff", "HEAD")
    names = _git(root, "diff", "HEAD", "--name-only", "-z")
    untracked = _git(root, "ls-files", "--others", "--exclude-standard", "-z")
    if head is None or diff is None or names is None or untracked is None:
        raise GateError("git state unavailable")
    changed = _splitz(names)
    entries: list[tuple[str, str]] = []
    for rel in _splitz(untracked):
        changed.append(rel)
        try:
            body = (Path(root) / rel).read_bytes()
        except OSError as e:
            raise GateError(f"cannot read untracked file {rel}: {e}") from e
        entries.append((rel, hashlib.sha256(body).hexdigest()))
    return TreeState(
        head=head.decode().strip(),
        diff_digest=hashlib.sha256(diff).hexdigest(),
        untracked=tuple(sorted(entries)),
        changed=tuple(sorted(changed)),
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
    """
    _scheme = getattr(sys.modules.get("gate"), "KEY_SCHEME", KEY_SCHEME)
    h = hashlib.sha256()
    h.update(_scheme)
    h.update(state.head.encode() + b"\0")
    h.update(state.diff_digest.encode() + b"\0")
    for rel, digest in state.untracked:
        h.update(f"U\0{rel}\0{digest}\0".encode())
    for key in ENV_KEYS:
        h.update(f"E\0{key}\0{_effective_env(env, key)}\0".encode())
    h.update(f"K\0{kind}\0".encode())
    h.update(b"C\0" + b"\0".join(arg.encode() for arg in argv) + b"\0")
    return h.hexdigest()


def cache_root() -> Path:
    base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "clio-gate"


def repo_key(root: str) -> str:
    return hashlib.sha256(str(Path(root).resolve()).encode()).hexdigest()[:16]


def entry_dir(root: str, fp: str) -> Path:
    return cache_root() / repo_key(root) / fp[:16]
