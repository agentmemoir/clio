from __future__ import annotations

import hashlib
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

from gate_classify import make_target_indexes, nextest_subcommand_index
from gate_kinds import ENV_KEYS, ENV_KEY_PREFIXES, GATE_SCOPE_DEFAULT, GATE_SCOPE_OFF, KEY_SCHEME

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


def _tool_identity(command: str, env: dict[str, str]) -> str:
    """Hash the bytes of the command a run would use.

    `shutil.which` refuses a file the host will not execute, and a `noexec`
    mount refuses every file. This key is about the bytes, so a command
    spelled as a path is fingerprinted from the file itself even when the
    mount rules out running it.
    """
    resolved = shutil.which(command, path=env.get("PATH"))
    if resolved is None and os.path.dirname(command) and Path(command).is_file():
        resolved = command
    if resolved is None:
        return f"missing:{command}:{env.get('PATH', '')}"
    path = Path(resolved).resolve()
    digest = hashlib.sha256()
    try:
        with path.open("rb") as executable:
            for chunk in iter(lambda: executable.read(1024 * 1024), b""):
                digest.update(chunk)
    except OSError as error:
        raise GateError(f"cannot fingerprint command executable {path}: {error}") from error
    return digest.hexdigest()


def _rustup_toolchain_identity(argv: list[str], env: dict[str, str]) -> str:
    """Hash selected toolchain binaries because rustup proxies stay unchanged."""
    rustup = shutil.which("rustup", path=env.get("PATH"))
    if rustup is None:
        return "rustup:missing"
    digest = hashlib.sha256()
    digest.update(os.fsencode(_tool_identity(rustup, env)))
    working_directory = _make_working_directories(argv)[-1]
    toolchain = ""
    if Path(argv[0]).name == "cargo" and len(argv) > 1 and argv[1].startswith("+"):
        toolchain = argv[1][1:]
    for tool in ("cargo", "rustc"):
        command = [rustup, "which"]
        if toolchain:
            command.extend(("--toolchain", toolchain))
        command.append(tool)
        try:
            result = subprocess.run(
                command,
                cwd=working_directory,
                env=env,
                capture_output=True,
                check=False,
                timeout=5,
            )
        except subprocess.TimeoutExpired as error:
            raise GateError(f"timed out resolving rustup {tool}") from error
        except OSError as error:
            raise GateError(f"cannot resolve rustup {tool}: {error}") from error
        if result.returncode:
            identity = f"unavailable:{result.returncode}:{os.fsdecode(result.stderr)}"
        else:
            path = Path(os.fsdecode(result.stdout).rstrip("\r\n")).resolve()
            identity = f"{path}:{_tool_identity(str(path), env)}"
        digest.update(os.fsencode(tool) + b"\0" + os.fsencode(identity) + b"\0")
    return digest.hexdigest()


def _make_working_directories(argv: list[str]) -> list[Path]:
    current = Path.cwd()
    found = [current]
    args = argv[1:]
    index = 0
    while index < len(args):
        argument = args[index]
        if argument in {"-C", "--directory"} and index + 1 < len(args):
            current = (current / args[index + 1]).resolve()
            found.append(current)
            index += 2
            continue
        if argument.startswith("-C") and argument != "-C":
            current = (current / argument[2:]).resolve()
            found.append(current)
        elif argument.startswith("--directory="):
            current = (current / argument.partition("=")[2]).resolve()
            found.append(current)
        index += 1
    return found


def _cargo_config_identity(argv: list[str], env: dict[str, str]) -> str:
    home = Path(env.get("HOME") or Path.home())
    cargo_home = Path(env.get("CARGO_HOME") or home / ".cargo")
    rustup_home = Path(env.get("RUSTUP_HOME") or home / ".rustup")
    files = {
        cargo_home / "config",
        cargo_home / "config.toml",
        cargo_home / "credentials.toml",
        rustup_home / "settings.toml",
        Path("/etc/rustup/settings.toml"),
    }
    for directory in _make_working_directories(argv):
        for ancestor in (directory, *directory.parents):
            files.add(ancestor / ".cargo" / "config")
            files.add(ancestor / ".cargo" / "config.toml")
            files.add(ancestor / "rust-toolchain")
            files.add(ancestor / "rust-toolchain.toml")
    digest = hashlib.sha256()
    for path in sorted(files, key=os.fspath):
        digest.update(os.fsencode(path) + b"\0")
        try:
            content = path.read_bytes()
        except FileNotFoundError:
            continue
        except OSError as error:
            raise GateError(f"cannot fingerprint tool configuration {path}: {error}") from error
        digest.update(hashlib.sha256(content).digest())
    return digest.hexdigest()


def fingerprint(state: TreeState, env: dict[str, str], kind: str, argv: list[str]) -> str:
    """The cache key: what makes one stored verdict distinguishable from another.

    The kind is hashed as well as the argument vector. The vector alone already
    separates the commands seen so far, but the kind is the narrower promise:
    it is what names the log the verdict is published under and which artifacts
    are saved and restored, so two commands that hash to one entry must also
    agree about that. Hashing it makes "a stored verdict is replayed only for
    the command it was measured from" a property of the key rather than of how
    carefully the argument vector happens to be spelled.

    Cargo and make use the content hash of the resolved executable, so path aliases to the same binary share a key and different binaries do not. Relevant environment values and command aliases are normalized below.
    """
    _scheme = getattr(sys.modules.get("gate"), "KEY_SCHEME", KEY_SCHEME)
    executable = Path(argv[0]).name
    h = hashlib.sha256()
    h.update(_scheme)
    h.update(state.head.encode() + b"\0")
    for rel, digest in state.contents:
        h.update(b"F\0" + os.fsencode(rel) + b"\0" + os.fsencode(digest) + b"\0")
    env_keys = set(ENV_KEYS)
    env_keys.update(key for key in env if key.startswith(ENV_KEY_PREFIXES))
    for key in sorted(env_keys):
        value = _effective_env(env, key)
        if key == "BASE" and executable == "make":
            assignments = [arg.partition("=")[2] for arg in argv[1:] if arg.startswith("BASE=")]
            if assignments:
                value = assignments[-1]
        elif key == "BASE":
            value = ""
        h.update(b"E\0" + os.fsencode(key) + b"\0" + os.fsencode(value) + b"\0")
    h.update(b"K\0" + os.fsencode(kind) + b"\0")
    args = argv[1:]
    if executable == "make":
        for target in make_target_indexes(argv):
            if argv[target] == "cov":
                args[target - 1] = "coverage"
        args = [arg for arg in args if not arg.startswith("BASE=")]
    elif executable == "cargo" and args[:1] == ["nextest"]:
        subcommand = nextest_subcommand_index(argv)
        if subcommand is not None:
            aliases = {"r": "run", "b": "bench"}
            argument = subcommand - 1
            args[argument] = aliases.get(args[argument], args[argument])
    normalized_argv = [executable if executable in {"cargo", "make"} else argv[0], *args]
    if executable in {"cargo", "make"}:
        h.update(b"T\0" + os.fsencode(_tool_identity(argv[0], env)) + b"\0")
        h.update(b"R\0" + os.fsencode(_rustup_toolchain_identity(argv, env)) + b"\0")
        h.update(b"Q\0" + os.fsencode(_cargo_config_identity(argv, env)) + b"\0")
    h.update(b"C\0" + b"\0".join(os.fsencode(arg) for arg in normalized_argv) + b"\0")
    return h.hexdigest()


def cache_root() -> Path:
    base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "clio-gate"


def repo_key(root: str) -> str:
    return hashlib.sha256(str(Path(root).resolve()).encode()).hexdigest()[:16]


def entry_dir(root: str, fp: str) -> Path:
    return cache_root() / repo_key(root) / fp[:16]
