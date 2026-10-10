#!/usr/bin/env python3
"""Content-addressed gate for heavy cargo and make commands in this repo.

Every workflow stage used to re-run the same suites on the same tree. This
wrapper fingerprints the working tree (HEAD, tracked diff, untracked files,
relevant environment, command kind, effective command), runs the command at most
once per fingerprint, serialises concurrent runs with a lock, and stores the
verdict (exit code and full log) for both pass and fail. A repeated command on an
unchanged tree returns the stored verdict instead of running again. A stored
failure is replayed too, so retrying the same tree does not reroll a flake;
a failure stored under a different PATH is re-run, so a broken environment
does not poison the tree. Edit the tree or pass --force for a fresh run. The
cache keeps the newest 50 verdicts per repository.

Every verdict is published under the name of the kind it was measured as, so a
log names the command that produced it. `cargo nextest` subcommands other than
`run` are stored and published as `nextest-lookup`, each derived coverage target
as itself, and a `cargo llvm-cov` line that measures nothing (a version probe,
`show-env`, `clean`) as `llvm-cov-lookup`. That is what stops a listing, a probe
or a derived report from standing in for a test run or a coverage report in the
log directory.

Quick scoping: `cargo test`, `cargo nextest run`, `cargo clippy`, and the
`make test`, `make check`, `make lint` targets are scoped to the changed
workspace crates and their reverse dependencies (make targets through
`PACKAGES`). Coverage runs are never scoped. A scoped result is labelled quick;
it is not a full-workspace verdict.

There is no full-workspace gate any more. `make coverage` and `cargo llvm-cov`
still produce a whole-workspace report and this wrapper still caches them, but
they judge nothing: the one coverage requirement is that every Rust file a change
created or modified reaches 80% of its lines, and `make coverage-fast` (its own
kind, so a replay is never labelled as the wider report) is what decides a
change. `make test-fast` is deliberately not routed
through this cache: a stored test verdict would replay a flake's pass, which is
the opposite of what a test gate is for.

The tree is fingerprinted again after the command; if it changed while the
command ran, the result is not cached.

`make -n` and its long forms are not cached. A dry run prints its recipes
instead of running them, so it produces no verdict to store, and storing one
would publish a printed recipe list under a real kind's name. The shim passes
those calls straight through, whether the flag arrives as an argument or through
`MAKEFLAGS`. A caller invoking `gate.py run` directly bypasses that, because the
shim is the only surface that sees the flag before the command runs.

Usage:
  python3 scripts/gate.py run [--force] [--log-dir DIR] -- CMD ARGS...
  python3 scripts/gate.py --self-test

Exit codes:
  the command's own exit code, on a fresh run or a reused verdict
  2   usage error
  70  gate internal error (callers should run the command directly)

Environment:
  GATE_RAW=1   bypass the gate entirely (handled by the shim, not here)
  GATE_FORCE=1 same as --force
  GATE_SCOPE=off  never scope a quick command
  CLIO_GATE_LOG_DIR  if the shim passes it, a copy of the newest log lands
                     there as <kind>.log plus <kind>.exit (used by workflows).
                     The run before the current one is kept beside them as
                     <kind>.prev: a header line naming the exit code that run
                     exited with, then its log.
  XDG_CACHE_HOME  cache root; defaults to ~/.cache
"""

from __future__ import annotations

import contextlib
import fcntl
import hashlib
import io
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path

# The shared rule for a temporary directory whose files the host will run lives
# beside this file. It is put on the path the same way the other suites that call
# it do it, so the import holds however this module is reached: run as a script,
# or imported by something that is not sitting in this directory.
sys.path.insert(0, str(Path(__file__).resolve().parent))

from exec_tempdir import exec_tempdir  # noqa: E402

EXIT_USAGE = 2
EXIT_INTERNAL = 70
LOG_TAIL_LINES = 12
#: Terminal formatting, stripped before anything is read out of a tool's help
#: text. Whether a tool colours its output is the caller's setting, so a value
#: read from it has to be the same on a CI runner and on a terminal.
ANSI_ESCAPE = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]")

CARGO_KINDS = {
    "test": "test",
    "nextest": "test",
    "llvm-cov": "coverage",
    "clippy": "clippy",
}
#: The `cargo nextest` subcommands that execute tests. Everything else under
#: `nextest` is a lookup or a utility -- `list`, `show-config`, `archive`,
#: `self` -- and nextest takes its subcommand before any flag, so the token after
#: `nextest` is the whole answer. `r` is nextest's own alias for `run`.
NEXTEST_EXECUTING_SUBCOMMANDS = frozenset({"run", "r"})
#: Every `cargo nextest` subcommand the tool advertises, aliases included, read
#: off its own `cargo nextest --help` at 0.9.146. This is not a list of options
#: that take a value, which is what an earlier version of this file carried and
#: which drifts every time the tool grows an option: the walk below needs to
#: recognise a subcommand, not to know which option swallowed the token in front
#: of it. A token that is not one of these is either an option's value or an
#: argument, and both are skipped, so an option this file has never heard of
#: cannot hide the subcommand behind it. A self-test compares this against the
#: installed tool, so a subcommand added upstream fails a check.
NEXTEST_KNOWN_SUBCOMMANDS = frozenset(
    {"list", "run", "r", "bench", "b", "archive", "show-config", "self", "help"}
)
#: The kind a non-executing `cargo nextest` subcommand is stored and published
#: under. It used to be `test`, because `CARGO_KINDS` maps the tool rather than
#: what the tool was asked to do, and that is what let the pre-commit hook which
#: compares the test lookup against the runner's own list of tests overwrite the
#: real test verdict and stand in for it. A reader could not tell a listing from
#: a run, the exit code of a lookup was served as the test result, and the log
#: tail printed on a failure came from the wrong command entirely. A verdict is
#: only readable if it names the command that produced it.
NEXTEST_LOOKUP_KIND = "nextest-lookup"
#: The `cargo llvm-cov` subcommands that produce a coverage report, read off its
#: own `cargo llvm-cov --help` at 0.9.0. `test`, `run` and `nextest` run something
#: and measure it; `report` measures the data an earlier run left behind.
LLVM_COV_REPORTING_SUBCOMMANDS = frozenset({"test", "run", "report", "nextest"})
#: Every `cargo llvm-cov` subcommand the tool advertises. The set has the same job
#: here that `NEXTEST_KNOWN_SUBCOMMANDS` has there: the walk below needs to
#: recognise a subcommand, not to know which option swallowed the token in front
#: of it, so an option added upstream cannot hide the subcommand behind it.
LLVM_COV_KNOWN_SUBCOMMANDS = frozenset(
    {"test", "run", "report", "show-env", "clean", "nextest"}
)
#: The kind a `cargo llvm-cov` invocation that measures nothing is stored and
#: published under. `CARGO_KINDS` maps the tool rather than what it was asked to
#: do, and that is what let a version or environment probe publish under the
#: `coverage` kind and overwrite a real coverage log: the reader was handed the
#: exit code of a probe as the coverage result. A verdict is only readable if it
#: names the command that produced it.
LLVM_COV_LOOKUP_KIND = "llvm-cov-lookup"
#: The flags that ask the tool to describe itself rather than measure anything.
#: They stand in for a subcommand, because `cargo llvm-cov --version` carries
#: none, and a bare `cargo llvm-cov` is `cargo llvm-cov test` in every way the
#: gate cares about.
LLVM_COV_PROBE_FLAGS = frozenset({"--version", "-V", "--help", "-h"})
MAKE_KINDS = {
    # `ritual` is the cheap local gate: the format check and the workspace-wide
    # clippy. It runs no test and judges no coverage, so a stored verdict for it
    # is safe to replay in a way a stored test verdict is not. It is listed here
    # so its verdict publishes under a stable kind rather than the generic
    # `make`, which is what the procedure files read out of logs/.
    # `verify` was the previous name for a gate that ran all four checks. It was
    # never in this table, so it was never cached and never published, which made
    # every stage file's claim that the wrapper covered it false. Both names were
    # checked when the target was renamed; this comment exists so the next
    # rename does not repeat it.
    "ritual": "ritual",
    "check": "check",
    "test": "test",
    "lint": "lint",
    "coverage": "coverage",
    "cov": "coverage",
    # A distinct kind from `coverage`, so a replayed verdict is never reported
    # as the full-workspace gate. `coverage-fast` judges only the changed files,
    # and its scope depends on the diff, which is part of the fingerprint.
    "coverage-fast": "coverage-fast",
    # Each of these is a distinct kind too, and for the same reason as
    # `coverage-fast` plus one more: they all used to publish under the plain
    # `coverage` name, so the log a reader is sent to after one of them ran held
    # that one's output instead of the coverage run's, and a report derived from
    # a report was indistinguishable from the report. A verdict is only readable
    # if it names the command that produced it. `cov` is left as an alias of
    # `coverage` because it asks for the same thing under a shorter name.
    "coverage-clean": "coverage-clean",
    "coverage-html": "coverage-html",
    "coverage-lcov": "coverage-lcov",
    "coverage-open": "coverage-open",
    # Measures one package into an instrumented directory of its own, and
    # judges nothing. A distinct kind for the same reason as `coverage-fast`: a
    # replayed verdict published under `coverage` would read as the
    # whole-workspace gate, and this is the opposite of that. The name contains
    # `coverage` on purpose, because that is what makes the self-test's coverage
    # registration assertion apply to it: that a target with `coverage` in its
    # name publishes a registered coverage kind. The other assertion reads every
    # key here, so this one is dispatched by the shim whatever it is called. A
    # diagnostic missing either would be neither cached, locked nor published
    # while looking like a target that had them.
    "coverage-package": "coverage-package",
}
#: Every kind that produces or consumes a coverage report, which is what a clean
#: has to discard: a report measures a build, and a clean removes the build, so a
#: stored verdict for one of these describes a tree that no longer exists. A kind
#: missing from here survives that clean, and its replay then serves a verdict
#: measured against a build that is gone.
#:
#: Registered does not mean this file owns that kind's report, and for most of
#: this set it does not. What a coverage kind saves is the single path
#: `COVERAGE_ARTIFACT` names, because a make target's argument vector never
#: carries `--output-path` -- the recipe that would hold it is inside the
#: Makefile, not in the command the wrapper is handed. Only `make coverage` writes
#: that path. The scoped gate writes a report of its own, a package run writes one
#: named after the package, and the lcov and html targets write neither, so for
#: those kinds this set buys the clean, the lock and the published log rather than
#: an artifact of their own.
COVERAGE_KINDS = frozenset(
    {
        "coverage",
        "coverage-fast",
        "coverage-clean",
        "coverage-html",
        "coverage-lcov",
        "coverage-open",
        "coverage-package",
    }
)
SCOPED_KINDS = {"test", "clippy"}
#: The two values the scoping switch has. It is a switch rather than a dial, so
#: only these are named: an unrecognised value behaves as the default does, and
#: the cache key records the side of the switch a command ran on rather than the
#: spelling used to say it.
GATE_SCOPE_DEFAULT = "quick"
GATE_SCOPE_OFF = "off"
#: The environment values that change what a command does, and so belong in the
#: cache key. This is an allowlist on purpose: hashing the whole environment
#: would put a timestamp or a shell session id in the key and every run would
#: miss. Each entry has to earn its place, because an entry that is missing
#: gives two different commands one stored verdict. A value read here to decide
#: what runs, and missing from this tuple, is the defect this file once had; a
#: self-test checks the two lists against each other.
ENV_KEYS = (
    "CARGO_BUILD_JOBS",
    "CARGO_INCREMENTAL",
    "CLIO_DEPLOYMENT_CONFIG",
    "CLIO_DATABASE_URL",
    "NEXTEST_PROFILE",
    "PROFILE",
    "RUSTC_WRAPPER",
    "RUSTFLAGS",
    # The two the scoping code below reads. Both change what runs while leaving
    # the argument vector untouched, which is exactly the shape of a key that is
    # too coarse: `PACKAGES` set in the environment makes `_scope_make` hand the
    # command back unrewritten, so `make test` with one package list and `make
    # test` with another were the same command line and shared one entry. The
    # second run was told the first run's exit code.
    "GATE_SCOPE",
    "PACKAGES",
)
#: Environment values this file reads that are deliberately not in the key, each
#: with the reason it cannot change what a command does. Anything read to decide
#: what runs belongs in `ENV_KEYS`; a self-test parses this file's own source
#: and fails on a name that is in neither list.
ENV_KEY_EXEMPT = {
    "XDG_CACHE_HOME": "chooses where the cache is kept, not what runs",
    "PATH": "a stored failure from another PATH is re-run rather than replayed",
    "GATE_FORCE": "decides only whether to consult the cache, and setting it skips it",
}
COVERAGE_ARTIFACT = "target/coverage/coverage.json"
#: The version salt mixed into every cache key. Bump it whenever the key stops
#: meaning what it meant, so an entry written under the old scheme cannot be read
#: back as if it had been written under the new one. Bazel calls this the action
#: cache salt and ccache the namespace; both use it the same way.
#:
#: v3 moved a `cargo llvm-cov` probe out of the `coverage` kind. An entry stored
#: before that was keyed on a kind the tool no longer publishes under, and it
#: would otherwise be readable as a coverage verdict.
KEY_SCHEME = b"clio-gate-v3\0"
USAGE = "usage: gate.py run [--force] [--log-dir DIR] -- CMD ARGS... | gate.py --self-test"


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
    h = hashlib.sha256()
    h.update(KEY_SCHEME)
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


def workspace_members(cargo: str, root: str) -> list[Member] | None:
    env = dict(os.environ)
    env["GATE_ACTIVE"] = "1"
    try:
        proc = subprocess.run(
            [cargo, "metadata", "--no-deps", "--locked", "--format-version", "1"],
            cwd=root,
            capture_output=True,
            env=env,
        )
    except OSError:
        return None
    if proc.returncode != 0:
        return None
    try:
        packages = json.loads(proc.stdout)["packages"]
    except (ValueError, KeyError, TypeError):
        return None
    by_dir = {str(Path(pkg["manifest_path"]).parent): pkg["name"] for pkg in packages}
    members = []
    for pkg in packages:
        deps = []
        for dep in pkg.get("dependencies", []):
            path = dep.get("path")
            if path and str(Path(path)) in by_dir:
                deps.append(by_dir[str(Path(path))])
            elif dep.get("source") is None and dep.get("name") in by_dir.values():
                deps.append(dep["name"])
        members.append(Member(pkg["name"], str(Path(pkg["manifest_path"]).parent), tuple(deps)))
    return members


def changed_seeds(root: str, changed: tuple[str, ...], members: list[Member]) -> set[str] | None:
    """Crates a change touches; None means the change is workspace-wide.

    A changed file that belongs to no crate used to abort scoping entirely, so
    editing a Makefile or a CI workflow silently rebuilt and reran the whole
    workspace. Two classes of such file are separated here:

    - Build inputs that genuinely reach every crate (`Cargo.toml`, `Cargo.lock`,
      the toolchain file, `.cargo/config`) still mean workspace-wide, because a
      dependency or profile change really can alter any crate.
    - Files that cannot change what any crate compiles (docs, `.gitignore`,
      editor settings) are ignored for scoping. They leave the seed set empty
      rather than forcing a full build.
    """
    if not changed:
        return set()
    prefixes = []
    for member in members:
        try:
            rel = str(Path(member.root).relative_to(root))
        except ValueError:
            continue
        prefixes.append((rel.rstrip("/") + "/", member.name))
    prefixes.sort(key=lambda pair: len(pair[0]), reverse=True)
    seeds: set[str] = set()
    for rel in changed:
        found = None
        for prefix, name in prefixes:
            if rel.startswith(prefix) or rel == prefix.rstrip("/"):
                found = name
                break
        if found is None:
            if _reaches_every_crate(rel):
                return None
            continue
        seeds.add(found)
    return seeds


def _reaches_every_crate(rel: str) -> bool:
    """True when a file outside every crate can still change what any crate builds.

    A shared manifest, a lockfile, or the toolchain file feeds every crate, so a
    change to one of those is workspace-wide by definition. Anything else outside
    the crate tree (a Makefile, a CI workflow, a document) reaches no crate's
    compilation, so it must not widen the scope.
    """
    if rel in ("Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml"):
        return True
    return rel.startswith(".cargo/")


def closure(seeds: set[str], members: list[Member]) -> list[str]:
    dependents: dict[str, set[str]] = {}
    for member in members:
        for dep in member.deps:
            dependents.setdefault(dep, set()).add(member.name)
    out = set(seeds)
    stack = list(seeds)
    while stack:
        for nxt in dependents.get(stack.pop(), ()):
            if nxt not in out:
                out.add(nxt)
                stack.append(nxt)
    return sorted(out)


def scope_command(
    kind: str, argv: list[str], root: str, changed: tuple[str, ...]
) -> tuple[list[str], list[str] | None]:
    if os.environ.get("GATE_SCOPE", GATE_SCOPE_DEFAULT) == GATE_SCOPE_OFF:
        return argv, None
    if Path(argv[0]).name == "make":
        return _scope_make(kind, argv, root, changed)
    if kind not in SCOPED_KINDS:
        return argv, None
    sub_index: int | None = None
    if Path(argv[0]).name == "cargo" and argv[1:2] == ["nextest"]:
        sub_index = nextest_subcommand_index(argv)
        if sub_index is None or argv[sub_index] not in NEXTEST_EXECUTING_SUBCOMMANDS:
            return argv, None
    if any(a in ("-p", "--package") or a.startswith("--package=") for a in argv):
        return argv, None
    if kind == "clippy" and any(a in ("--workspace", "--all") for a in argv):
        # A lint run that named the whole workspace asked for the whole
        # workspace. Narrowing it would publish a verdict over a handful of crates
        # under the lint's own name, and a lint that skipped crates reports
        # success -- which is the failure this exemption exists to stop. The
        # command printed in the pull request template is exactly such a command.
        #
        # Clippy only. `make lint` is scoped on purpose and is scoped through
        # PACKAGES by `_scope_make`, which is untouched, and a narrowed test run
        # is the documented local fast path for every kind the wrapper scopes. A
        # lint is the one scoped kind whose narrowed result is read as a verdict
        # about the whole checkout.
        return argv, None
    members = workspace_members(argv[0], root)
    if not members:
        return argv, None
    seeds = changed_seeds(root, changed, members)
    if not seeds:
        # No crate is affected, so there is nothing to narrow to and nothing for
        # cargo to rebuild. Running unscoped is cheap here.
        return argv, None
    packages = closure(seeds, members)
    filtered = [a for a in argv if a not in ("--workspace", "--all")]
    # A package flag goes after the subcommand for `cargo nextest`, and straight
    # after the tool for the others, which take no subcommand of their own. The
    # subcommand is located by walking, not by counting tokens, because nextest
    # takes its own options first: a fixed index put the flag in front of the
    # subcommand, which is not a legal command at all.
    if sub_index is None:
        insert_at = min(2, len(filtered))
    else:
        insert_at = min(filtered.index(argv[sub_index]) + 1, len(filtered))
    flags: list[str] = []
    for pkg in packages:
        flags += ["-p", pkg]
    return filtered[:insert_at] + flags + filtered[insert_at:], packages


def _scope_make(
    kind: str, argv: list[str], root: str, changed: tuple[str, ...]
) -> tuple[list[str], list[str] | None]:
    """Scope `make test`, `make check`, and `make lint` through PACKAGES."""
    if kind not in ("test", "check", "lint"):
        return argv, None
    if os.environ.get("PACKAGES") or any(arg.startswith("PACKAGES=") for arg in argv[1:]):
        return argv, None
    members = workspace_members("cargo", root)
    if not members:
        return argv, None
    seeds = changed_seeds(root, changed, members)
    if not seeds:
        return argv, None
    packages = closure(seeds, members)
    flags = " ".join(f"-p {pkg}" for pkg in packages)
    return [*argv, f"PACKAGES={flags}"], packages


def nextest_subcommand_index(argv: list[str]) -> int | None:
    """Where the `cargo nextest` subcommand sits in argv, or None if unreadable.

    nextest puts its own options before the subcommand, so the token straight
    after `nextest` is not always the subcommand, and `cargo nextest --verbose
    run` is a real command that runs every test in the workspace. Reading one
    token is wrong for it; knowing which options swallow a token is wrong as
    soon as the tool grows one.

    So the walk looks for a token this file recognises as a subcommand and skips
    everything else. An option's value and a filter argument are both skipped
    without needing to tell them apart, because neither can be mistaken for
    something the walk is looking for.

    None is returned rather than a guess when no subcommand can be found. An
    unreadable command must not be reported as a test run: that is the mistake
    this whole classification exists to stop. A filter past `--` may be spelled
    like a subcommand, so the walk stops there.
    """
    for index in range(2, len(argv)):
        token = argv[index]
        if token == "--":
            return None
        if token in NEXTEST_KNOWN_SUBCOMMANDS:
            return index
    return None


def nextest_subcommand(argv: list[str]) -> str:
    """The `cargo nextest` subcommand, or "" when the command line does not say."""
    index = nextest_subcommand_index(argv)
    return argv[index] if index is not None else ""


def llvm_cov_subcommand_index(argv: list[str]) -> int | None:
    """Where the `cargo llvm-cov` subcommand sits in argv, or None if unreadable.

    llvm-cov writes `cargo llvm-cov [SUBCOMMAND] [OPTIONS]`, so its subcommand is
    meant to come first, but an option may precede it and the walk here is the
    same one `nextest_subcommand_index` uses: skip anything this file does not
    recognise and take the first token that is a subcommand. That way an option
    added upstream, or one taking a value this file has never heard of, cannot
    hide the subcommand behind it.

    The walk stops at `--`, because everything past it is an argument for the
    test binary and a filter is allowed to be spelled exactly like a subcommand.

    None is returned rather than a guess when no subcommand can be found. A bare
    `cargo llvm-cov` is `cargo llvm-cov test` in every way this gate cares about,
    so an unreadable line is left to the caller to judge as a coverage run
    rather than being reported as a probe.
    """
    for index in range(2, len(argv)):
        token = argv[index]
        if token == "--":
            return None
        if token in LLVM_COV_KNOWN_SUBCOMMANDS:
            return index
    return None


def llvm_cov_measures_coverage(argv: list[str]) -> bool:
    """Whether this `cargo llvm-cov` line produces a coverage report.

    Three shapes measure and two do not. A subcommand that measures, and a bare
    `cargo llvm-cov` with no subcommand at all, both produce a report: the bare
    form is documented as equivalent to `cargo llvm-cov test`. Everything else
    does not: `show-env` prints the environment, `clean` removes the artifacts,
    and a self-describing flag asks the tool about itself.

    A flag that describes the tool is checked across the whole line rather than
    only when no subcommand was found, because the two can both be present:
    `cargo llvm-cov --version report` asks the tool its version, writes no report
    and exits 0, and reading the subcommand first would publish that as a
    coverage verdict. The scan stops at `--`, because a filter past it belongs to
    the test binary, so a token spelled like a flag there is an argument.

    The limit, stated rather than hidden: a subcommand this file has never heard
    of reads as a bare run, so it is classified as a coverage run. That is the
    opposite of the nextest rule, and deliberately so. `cargo nextest` alone is an
    error, so an unrecognised token there can only be a subcommand this file has
    not caught up with. `cargo llvm-cov` alone is a coverage run, and its options
    take bare values -- `--output-path reports/out.json` is a report, not a
    subcommand -- so treating any unrecognised bare token as a subcommand would
    classify real reports as probes and lose their artifacts. A self-test
    compares the subcommand list against the installed tool, so an upstream
    addition fails a check rather than being silently misclassified.
    """
    for token in argv[2:]:
        if token == "--":
            break
        if token in LLVM_COV_PROBE_FLAGS:
            return False
    index = llvm_cov_subcommand_index(argv)
    if index is not None:
        return argv[index] in LLVM_COV_REPORTING_SUBCOMMANDS
    return True


def command_kind(argv: list[str]) -> str:
    base = Path(argv[0]).name
    if base == "make":
        for arg in argv[1:]:
            if arg in MAKE_KINDS:
                return MAKE_KINDS[arg]
        return "make"
    if base == "cargo" and len(argv) > 1:
        if argv[1] == "nextest":
            # What the tool was asked to do, not which tool was called. Only
            # `run` executes the test suite, and the pre-commit hook runs the
            # listing one, so classifying by tool alone let a listing stand in
            # for a run's verdict. A subcommand this file does not know lands
            # here too, which is the safe side: it is published under its own
            # name rather than claiming to be a test run.
            if nextest_subcommand(argv) in NEXTEST_EXECUTING_SUBCOMMANDS:
                return CARGO_KINDS["nextest"]
            return NEXTEST_LOOKUP_KIND
        if argv[1] == "llvm-cov":
            # The same distinction, for the same reason. `CARGO_KINDS` maps the
            # tool rather than what it was asked to do, so a version probe or an
            # environment dump used to publish under `coverage` and overwrite a
            # real coverage log with the exit code of a probe.
            #
            # A subcommand this file does not know does *not* land in the lookup
            # kind here, unlike nextest. See `llvm_cov_measures_coverage` for why
            # the safe side is the other one for this tool, and for what makes an
            # unrecognised subcommand fail a check rather than change behaviour.
            if llvm_cov_measures_coverage(argv):
                return CARGO_KINDS["llvm-cov"]
            return LLVM_COV_LOOKUP_KIND
        return CARGO_KINDS.get(argv[1], "cargo")
    return "command"


def acquire_lock(root: str) -> int:
    lock_path = cache_root() / repo_key(root) / "lock"
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(lock_path, os.O_CREAT | os.O_RDWR, 0o644)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        print("gate: another run holds the lock; waiting...", file=sys.stderr)
        fcntl.flock(fd, fcntl.LOCK_EX)
    return fd


def _artifact_rel_paths(argv: list[str], kind: str) -> list[str]:
    found: list[str] = []
    want_next = False
    for arg in argv:
        if want_next:
            found.append(arg)
            want_next = False
        elif arg == "--output-path":
            want_next = True
        elif arg.startswith("--output-path="):
            found.append(arg.split("=", 1)[1])
    if kind in COVERAGE_KINDS:
        found.append(COVERAGE_ARTIFACT)
    return list(dict.fromkeys(found))


def _blob_name(rel: str) -> str:
    return hashlib.sha256(rel.encode()).hexdigest()[:12] + "-" + Path(rel).name


def _save_artifacts(entry: Path, root: str, argv: list[str], kind: str) -> list[str]:
    saved = []
    for rel in _artifact_rel_paths(argv, kind):
        src = Path(root) / rel
        if not src.is_file():
            continue
        dest = entry / "artifacts" / _blob_name(rel)
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(src.read_bytes())
        saved.append(rel)
    return saved


def _restore_artifacts(entry: Path, root: str, meta: dict) -> None:
    """Put back an artifact a replay would otherwise leave missing.

    A replay does not run the command, so a file the command wrote is not written
    again, and this is what puts it back.

    It never replaces a file that is already there. The copy in the cache is a
    snapshot taken at the end of an earlier run, while a report lives under
    `target/` and so is not part of the fingerprint a replay matches on. Writing
    the older copy over a newer one would leave the reader holding a measurement
    of a different tree, with a green verdict beside it and nothing to say the two
    do not match. An empty live file is the one exception: it holds no
    measurement at all, which is what a shell redirect or an interrupted write
    leaves behind, so restoring over it is a repair.
    """
    for rel in meta.get("artifacts", []):
        src = entry / "artifacts" / _blob_name(rel)
        if not src.is_file():
            continue
        live = Path(root) / rel
        if live.is_file() and live.stat().st_size:
            print(f"gate: kept {rel}; a file is already there and this replay measured nothing")
            continue
        live.parent.mkdir(parents=True, exist_ok=True)
        live.write_bytes(src.read_bytes())
        print(f"gate: restored {rel} from cache")


def _purge_coverage_cache(root: str) -> None:
    repo_cache = cache_root() / repo_key(root)
    if not repo_cache.is_dir():
        return
    removed = 0
    for meta_path in repo_cache.glob("*/meta.json"):
        try:
            meta = json.loads(meta_path.read_text())
        except (OSError, ValueError):
            continue
        if isinstance(meta, dict) and meta.get("kind") in COVERAGE_KINDS:
            shutil.rmtree(meta_path.parent, ignore_errors=True)
            removed += 1
    if removed:
        print(f"gate: cleared {removed} cached coverage verdict(s) after coverage-clean")


def _prune_cache(root: str, keep: int = 50) -> None:
    repo_cache = cache_root() / repo_key(root)
    if not repo_cache.is_dir():
        return
    metas = sorted(
        repo_cache.glob("*/meta.json"), key=lambda path: path.stat().st_mtime, reverse=True
    )
    for stale in (meta.parent for meta in metas[keep:]):
        shutil.rmtree(stale, ignore_errors=True)


def _print_tail(log_path: Path) -> None:
    try:
        lines = log_path.read_text(errors="replace").splitlines()
    except OSError:
        return
    shown = lines[-LOG_TAIL_LINES:]
    print(f"gate: last {len(shown)} log lines:")
    for line in shown:
        print(f"gate| {line}")


def _publish_log(log_path: Path, log_dir: str, kind: str, code: int) -> Path | None:
    """Publish the newest run's log and keep the run it replaces.

    Three files, and only the first two existed before:

    * ``<kind>.log`` and ``<kind>.exit`` -- the newest run, under the names every
      reader and every recorded citation already uses.
    * ``<kind>.prev`` -- the run before it, as **one** file: a header line naming
      the exit code that run exited with, then its log.

    The kept run is one file rather than a pair because two files cannot be
    swapped without a window in which the log and the exit code describe different
    runs, and a record carrying the wrong exit code is worse than no record. One
    file written to a staging path and renamed into place has no such window: the
    rename is the only step that becomes visible.

    A file already at the published name is kept only when it is non-empty and
    different. An empty one is what a shell redirect leaves behind, so it is the
    common case and not a corner: preserving it would overwrite the previous
    run's record with nothing and lose the evidence this function exists to keep.
    Republishing identical bytes is a replay rather than a new run, and rotates
    nothing, so a replay cannot file a copy of the current log as an earlier one.
    Two genuinely different runs whose output happens to be identical are
    therefore not separated; that is accepted, because inventing a false earlier
    run is the worse error for an evidence store.

    A log that cannot be read is not overwritten on the strength of a failure to
    read it: the read is guarded separately so an unreadable leftover costs the
    kept run and nothing else.
    """
    if not log_dir:
        return None
    try:
        dest_dir = Path(log_dir)
        dest_dir.mkdir(parents=True, exist_ok=True)
        dest = dest_dir / f"{kind}.log"
        exit_file = dest_dir / f"{kind}.exit"
        fresh = log_path.read_bytes()
        existing = b""
        if dest.is_file():
            try:
                existing = dest.read_bytes()
            except OSError:
                existing = b""
        if existing and existing != fresh:
            try:
                carried = exit_file.read_text().strip() or "unknown"
            except OSError:
                carried = "unknown"
            staged = dest_dir / f".{kind}.prev.staged"
            try:
                staged.write_bytes(f"exit: {carried}\n".encode() + existing)
                staged.replace(dest_dir / f"{kind}.prev")
            finally:
                staged.unlink(missing_ok=True)
        dest.write_bytes(fresh)
        exit_file.write_text(f"{code}\n")
        return dest
    except OSError as e:
        print(f"gate: warning: cannot publish log to {log_dir}: {e}", file=sys.stderr)
        return None


def _reuse(entry: Path, meta: dict, log_dir: str) -> int:
    code = int(meta["exit"])
    print(
        f"gate: reusing stored {meta['kind']} verdict for tree {meta['tree'][:12]}: "
        f"exit {code} ({meta.get('seconds', '?')}s)"
    )
    log_path = entry / meta.get("log", "log")
    published = _publish_log(log_path, log_dir, meta["kind"], code)
    print(f"gate: log: {published or log_path}")
    if code != 0:
        _print_tail(log_path)
    return code


def run_gate(argv: list[str], force: bool = False, log_dir: str = "") -> int:
    cwd = os.getcwd()
    root = repo_root(cwd)
    if root is None:
        raise GateError("not inside a git checkout")
    lock_fd = acquire_lock(root)
    try:
        state = read_tree_state(root)
        kind = command_kind(argv)
        if Path(argv[0]).name == "cargo" and argv[1:2] == ["nextest"] and not nextest_subcommand(argv):
            # Loud on purpose. A subcommand this file does not recognise is
            # classified as a lookup rather than guessed at, which is the safe
            # answer but quietly loses quick scoping and publishes under a name
            # that may not describe the command. Saying so turns a drift in the
            # tool's own command line into something a reader can see.
            print(
                f"gate: warning: no known cargo nextest subcommand in {shlex.join(argv)}; "
                f"treating it as {kind}, which may not be what it does",
                file=sys.stderr,
            )
        # A command that removes build output must run every time, never replay from a
        # stored verdict: a replay would report success without having done the
        # removal. `make coverage-clean` is named as a target, and `cargo llvm-cov
        # clean` is the same request to the other tool. `clean` is a lookup kind,
        # not a coverage one, so it is not in COVERAGE_KINDS and does not need the
        # artifact handling that set buys.
        clean_requested = "coverage-clean" in argv[1:] or (
            len(argv) > 2 and argv[1:3] == ["llvm-cov", "clean"]
        )
        if clean_requested:
            force = True
        scoped_argv, scope = scope_command(kind, argv, root, state.changed)
        fp = fingerprint(state, os.environ, kind, scoped_argv)
        entry = entry_dir(root, fp)
        meta_path = entry / "meta.json"
        if meta_path.is_file() and not force:
            try:
                meta = json.loads(meta_path.read_text())
            except (OSError, ValueError):
                meta = None
            if isinstance(meta, dict) and meta.get("schema") == 1 and meta.get("tree") == fp:
                fresh_env = meta.get("exit", 1) == 0 or meta.get("path") == os.environ.get("PATH", "")
                if fresh_env:
                    _restore_artifacts(entry, root, meta)
                    return _reuse(entry, meta, log_dir)
                print("gate: stored failure came from a different PATH; re-running", file=sys.stderr)
        entry.mkdir(parents=True, exist_ok=True)
        log_path = entry / "log"
        if scope:
            print(f"gate: quick scoped run ({kind}) on changed crates: {', '.join(scope)}")
            print("gate: not a full-workspace verdict; run `make coverage` before claiming done")
        print(f"gate: running {shlex.join(scoped_argv)}")
        started = time.time()
        env = dict(os.environ)
        env["GATE_ACTIVE"] = "1"
        with open(log_path, "wb") as log:
            code = subprocess.call(scoped_argv, cwd=cwd, env=env, stdout=log, stderr=subprocess.STDOUT)
        finished = time.time()
        published = _publish_log(log_path, log_dir, kind, code)
        if clean_requested:
            _purge_coverage_cache(root)
            print(f"gate: {kind} exit {code} in {round(finished - started, 1)}s; log: {published or log_path}")
            if code != 0:
                _print_tail(log_path)
            return code
        if fingerprint(read_tree_state(root), os.environ, kind, scoped_argv) != fp:
            print("gate: tree changed while the command ran; result not cached", file=sys.stderr)
            print(f"gate: log: {published or log_path}")
            return code
        meta = {
            "schema": 1,
            "tree": fp,
            "kind": kind,
            "argv": scoped_argv,
            "scope": scope,
            "exit": code,
            "path": os.environ.get("PATH", ""),
            "started": started,
            "finished": finished,
            "seconds": round(finished - started, 1),
            "artifacts": _save_artifacts(entry, root, scoped_argv, kind),
            "log": "log",
        }
        tmp = entry / "meta.json.tmp"
        tmp.write_text(json.dumps(meta, indent=2) + "\n")
        os.replace(tmp, meta_path)
        _prune_cache(root)
        print(f"gate: {kind} exit {code} in {meta['seconds']}s; log: {published or log_path}")
        if code != 0:
            _print_tail(log_path)
        return code
    finally:
        os.close(lock_fd)


def _self_test() -> int:
    import unittest
    from unittest import mock

    def state(head: str, diff: str = "d", untracked=(), changed=()) -> TreeState:
        return TreeState(head, diff, tuple(untracked), tuple(changed))

    def published(name: str, log_dir: Path) -> str:
        """What the wrapper published under a kind, or "" when it published nothing.

        A missing log is the failure these checks are looking for, so it has to
        come back as a value the comparison rejects rather than as an exception
        that hides which check failed.
        """
        try:
            return (log_dir / name).read_text()
        except OSError:
            return ""

    skipped: list[str] = []

    members = [
        Member("a", "/repo/crates/a", ("b",)),
        Member("b", "/repo/crates/b", ()),
        Member("c", "/repo/crates/c", ()),
    ]

    class Tests(unittest.TestCase):
        def test_fingerprint_stable_and_sensitive(self):
            env = {"CLIO_DATABASE_URL": "x"}
            touched = ("crates/a/src/lib.rs",)
            baseline = fingerprint(state("h", changed=touched), env, "test", ["cargo", "test"])
            self.assertEqual(baseline, fingerprint(state("h", changed=touched), env, "test", ["cargo", "test"]))
            self.assertNotEqual(baseline, fingerprint(state("h2", changed=touched), env, "test", ["cargo", "test"]))
            self.assertNotEqual(baseline, fingerprint(state("h", diff="d2", changed=touched), env, "test", ["cargo", "test"]))
            self.assertNotEqual(
                baseline,
                fingerprint(state("h", changed=touched, untracked=(("new.rs", "z"),)), env, "test", ["cargo", "test"]),
            )
            self.assertNotEqual(baseline, fingerprint(state("h", changed=touched), {"CLIO_DATABASE_URL": "y"}, "test", ["cargo", "test"]))
            self.assertNotEqual(baseline, fingerprint(state("h", changed=touched), env, "test", ["cargo", "test", "--workspace"]))
            # The kind is hashed as well as the command, so the name a verdict is
            # published under is part of what the entry is keyed on rather than
            # something decided after the lookup.
            self.assertNotEqual(baseline, fingerprint(state("h", changed=touched), env, "coverage", ["cargo", "test"]))

        def test_the_package_list_from_the_environment_separates_entries(self):
            # Two `make test` runs that differ only in the package list the
            # environment supplies were one command line, so the second was told
            # the first one's exit code. The scoping code reads both of these,
            # which is why they belong in the key.
            touched = ("crates/a/src/lib.rs",)
            argv = ["make", "test"]
            one = fingerprint(state("h", changed=touched), {"PACKAGES": "-p a"}, "test", argv)
            other = fingerprint(state("h", changed=touched), {"PACKAGES": "-p b"}, "test", argv)
            self.assertNotEqual(one, other)
            for key in ("PACKAGES", "GATE_SCOPE"):
                self.assertIn(key, ENV_KEYS)
            # An unset scoping switch and its default name are one command, and
            # so is a value the code never acts on, because the switch has two
            # sides rather than a range.
            for spelling in ({}, {"GATE_SCOPE": "quick"}, {"GATE_SCOPE": "junk"}, {"GATE_SCOPE": ""}):
                self.assertEqual(
                    fingerprint(state("h", changed=touched), {}, "test", argv),
                    fingerprint(state("h", changed=touched), spelling, "test", argv),
                    spelling,
                )
            self.assertNotEqual(
                fingerprint(state("h", changed=touched), {}, "test", argv),
                fingerprint(state("h", changed=touched), {"GATE_SCOPE": "off"}, "test", argv),
            )
            # The default the key normalises to and the default the code branches
            # on are one value, not two copies of a literal that can drift.
            self.assertEqual(GATE_SCOPE_DEFAULT, "quick")
            self.assertEqual(GATE_SCOPE_OFF, "off")

        def test_every_environment_read_that_changes_a_run_is_in_the_key(self):
            # The one collision this branch fixed was a variable the scoping code
            # read and the key ignored. Naming the two variables that happened to
            # cause it restates the fix; parsing this file's own source for every
            # name it reads out of the environment states the rule, so the next
            # such variable fails here rather than in a run that never happened.
            source = Path(__file__).read_text()
            reads = set(re.findall(r'os\.environ(?:\.get\(|\[)\s*"([A-Z_][A-Z0-9_]*)"', source))
            self.assertIn("GATE_SCOPE", reads, "the scan found nothing and is not testing anything")
            for name in sorted(reads - set(ENV_KEYS)):
                self.assertIn(
                    name,
                    ENV_KEY_EXEMPT,
                    f"read from the environment to decide what runs, but absent from ENV_KEYS: {name}. "
                    "Two commands differing only in it would share one stored verdict.",
                )
            for name, reason in ENV_KEY_EXEMPT.items():
                self.assertNotEqual("", reason.strip(), f"{name} is exempt with no stated reason")

        def test_the_key_scheme_is_hashed_and_tracked(self):
            # Two separate promises. The salt has to reach the digest, or an
            # entry written under an older key would be read back under a newer
            # one; and the salt has to be recorded, or moving it is invisible.
            touched = ("crates/a/src/lib.rs",)
            env = {}
            kind = "test"
            argv = ["cargo", "test"]
            baseline = fingerprint(state("h", changed=touched), env, kind, argv)
            with mock.patch("__main__.KEY_SCHEME", b"clio-gate-some-other-scheme\0"):
                self.assertNotEqual(
                    baseline, fingerprint(state("h", changed=touched), env, kind, argv)
                )
            self.assertEqual(b"clio-gate-v3\0", KEY_SCHEME)
            # The version is the second, so an entry from the scheme before this
            # one cannot be mistaken for one from this one.
            self.assertTrue(KEY_SCHEME.endswith(b"v3\0"))

        def test_a_test_lookup_is_not_a_test_verdict(self):
            self.assertEqual("test", command_kind(["cargo", "nextest", "run"]))
            self.assertEqual("test", command_kind(["cargo", "nextest", "r"]))
            for argv in (
                ["cargo", "nextest", "list", "--workspace", "--locked"],
                ["cargo", "nextest", "show-config"],
                ["cargo", "nextest", "archive", "--workspace"],
                # `bench` executes benchmark binaries, so it is not a lookup in
                # the ordinary sense. It is bucketed with them anyway because it
                # does not run the test suite, and a benchmark's verdict must
                # never be able to stand in for one.
                ["cargo", "nextest", "bench"],
                ["cargo", "nextest", "help"],
                # Nothing to read, so nothing is claimed.
                ["cargo", "nextest"],
                ["cargo", "nextest", "--help"],
                # A subcommand added upstream that this file has never heard of
                # must not be reported as a run.
                ["cargo", "nextest", "some-future-subcommand"],
            ):
                self.assertEqual(NEXTEST_LOOKUP_KIND, command_kind(argv), argv)
            self.assertNotIn(NEXTEST_LOOKUP_KIND, SCOPED_KINDS)

        def test_the_nextest_subcommand_survives_its_own_options(self):
            # nextest puts its options before the subcommand, so the token after
            # `nextest` is not always the subcommand. These are the shapes that
            # make reading one token wrong.
            self.assertEqual("run", nextest_subcommand(["cargo", "nextest", "run", "-p", "a"]))
            self.assertEqual("run", nextest_subcommand(["cargo", "nextest", "--verbose", "run"]))
            self.assertEqual("run", nextest_subcommand(["cargo", "nextest", "-v", "run"]))
            self.assertEqual("run", nextest_subcommand(["cargo", "nextest", "--no-pager", "run"]))
            self.assertEqual("run", nextest_subcommand(["cargo", "nextest", "--color", "always", "run"]))
            self.assertEqual("run", nextest_subcommand(["cargo", "nextest", "--color=always", "run"]))
            self.assertEqual("run", nextest_subcommand(["cargo", "nextest", "-P", "ci", "run"]))
            self.assertEqual("list", nextest_subcommand(["cargo", "nextest", "--manifest-path", "Cargo.toml", "list"]))
            # The shape a list of options taking values was read to handle, and
            # the one it could not survive: an option that takes a value this
            # file has never heard of. The value is skipped because it is not a
            # subcommand, so the walk never needed to know the option existed.
            self.assertEqual("run", nextest_subcommand(["cargo", "nextest", "--new-upstream-option", "VALUE", "run"]))
            self.assertEqual("test", command_kind(["cargo", "nextest", "--new-upstream-option", "VALUE", "run"]))
            self.assertEqual("", nextest_subcommand(["cargo", "nextest"]))
            # A filter after `--` may be spelled like a subcommand and must not
            # be read as one.
            self.assertEqual("run", nextest_subcommand(["cargo", "nextest", "run", "--", "list"]))
            self.assertEqual("", nextest_subcommand(["cargo", "nextest", "--", "list"]))
            for shape in (["cargo", "nextest", "--verbose", "run"], ["cargo", "nextest", "--color", "always", "run"]):
                self.assertEqual("test", command_kind(shape), shape)

        def test_the_known_subcommands_match_the_installed_nextest(self):
            # The list is read off the tool's own help, and nextest adds
            # subcommands. One this file has never heard of is classified as a
            # lookup, which is the safe answer but silently loses quick scoping
            # and publishes under a name that may not describe the command, so a
            # new subcommand has to fail a check rather than change behaviour.
            #
            # The real tool is invoked with GATE_ACTIVE set, which is what the
            # shim looks for to pass straight through, so asking the question
            # cannot itself be cached, locked or replayed.
            proc = subprocess.run(
                ["cargo", "nextest", "--help"],
                capture_output=True,
                text=True,
                check=False,
                env=dict(os.environ, GATE_ACTIVE="1"),
            )
            if proc.returncode != 0 or "Commands:" not in proc.stdout:
                self.skipTest("cargo nextest is not installed on this machine")
            # Whether the tool colours its help is the caller's decision, and CI
            # sets CARGO_TERM_COLOR=always, so the names arrive wrapped in escape
            # sequences there and bare on a terminal. Stripping the formatting is
            # what makes this check read the same names on both, rather than
            # asserting a property of whichever machine ran it.
            plain = ANSI_ESCAPE.sub("", proc.stdout)
            listed: set[str] = set()
            # The heading is followed by a newline of its own, so the remainder
            # is split from the first line that carries a name.
            body = plain.partition("Commands:")[2].lstrip("\n")
            for line in body.splitlines():
                if not line.strip():
                    break
                parts = line.split()
                if not parts:
                    continue
                listed.add(parts[0])
                # `run` is annotated `[alias: r]`, so an alias is a name this
                # file has to recognise too or an aliased call reads as
                # unreadable.
                for alias in re.findall(r"\[alias:\s*([^\]\s]+)\]", line):
                    listed.add(alias)
            self.assertTrue(listed, "the help output was parsed into nothing")
            for name in sorted(listed):
                self.assertEqual(name, name.strip(), f"{name!r} carries formatting into the comparison")
            self.assertEqual(
                sorted(listed),
                sorted(NEXTEST_KNOWN_SUBCOMMANDS),
                "the subcommands this file recognises no longer match the installed tool. "
                "Anything new is classified as a lookup rather than a run, so add it to "
                "NEXTEST_KNOWN_SUBCOMMANDS, and to NEXTEST_EXECUTING_SUBCOMMANDS if it runs tests.",
            )
            self.assertTrue(
                NEXTEST_EXECUTING_SUBCOMMANDS <= listed,
                "this file claims a subcommand runs tests that the tool does not offer",
            )

        def test_a_recognised_subcommand_is_never_taken_from_behind_a_separator(self):
            # The walk stops at `--` because everything past it is the
            # subcommand's own argument, and a filter is allowed to be spelled
            # exactly like a subcommand. Reading one would classify a filtered
            # run as a lookup.
            self.assertIsNone(nextest_subcommand_index(["cargo", "nextest", "--", "run"]))
            self.assertEqual(2, nextest_subcommand_index(["cargo", "nextest", "run", "--", "list"]))
            # An option's value may also be spelled like a subcommand, and there
            # is no way to tell that from a real one. The first match wins,
            # because a subcommand always precedes its own arguments, so the
            # only way to reach this is an option whose value is spelled exactly
            # like a subcommand name.
            self.assertEqual(3, nextest_subcommand_index(["cargo", "nextest", "--color", "list", "run"]))

        def test_a_coverage_probe_is_not_a_coverage_verdict(self):
            # The defect this classifies: `CARGO_KINDS` maps the tool, so
            # `cargo llvm-cov --version` published under `coverage` and
            # overwrote a real coverage log with the exit code of a version
            # probe. Every shape that measures nothing has to land in the
            # lookup kind, and every shape that measures has to stay out of it.
            for argv in (
                # A version probe, which is the shape the plan names.
                ["cargo", "llvm-cov", "--version"],
                ["cargo", "llvm-cov", "-V"],
                # An environment dump, the other one named.
                ["cargo", "llvm-cov", "show-env"],
                # Removes the artifacts and reports nothing.
                ["cargo", "llvm-cov", "clean"],
                ["cargo", "llvm-cov", "clean", "--workspace"],
                # Describing itself.
                ["cargo", "llvm-cov", "--help"],
                # A probe flag and a subcommand in the same line. The tool
                # answers the flag and writes no report, so reading the
                # subcommand first would publish its exit code as a coverage
                # verdict, which is the defect this whole classification exists
                # to stop.
                ["cargo", "llvm-cov", "--version", "report"],
                ["cargo", "llvm-cov", "--help", "test"],
            ):
                self.assertEqual(LLVM_COV_LOOKUP_KIND, command_kind(argv), argv)
            self.assertNotIn(LLVM_COV_LOOKUP_KIND, COVERAGE_KINDS)
            self.assertNotIn(LLVM_COV_LOOKUP_KIND, SCOPED_KINDS)
            # The other side. A bare `cargo llvm-cov` is documented as
            # equivalent to `cargo llvm-cov test`, so it measures and must stay
            # a coverage run; a probe must not have narrowed that.
            for argv in (
                ["cargo", "llvm-cov"],
                ["cargo", "llvm-cov", "--workspace"],
                ["cargo", "llvm-cov", "--summary-only"],
                ["cargo", "llvm-cov", "test", "--workspace"],
                ["cargo", "llvm-cov", "run"],
                ["cargo", "llvm-cov", "nextest", "--workspace"],
                ["cargo", "llvm-cov", "report"],
            ):
                self.assertEqual("coverage", command_kind(argv), argv)
            # Coverage is never narrowed, whatever produced it.
            for argv in (["cargo", "llvm-cov", "report"], ["cargo", "llvm-cov", "test"]):
                self.assertEqual(
                    (argv, None), scope_command(command_kind(argv), argv, "/repo", ("crates/b/src/lib.rs",))
                )

        def test_the_llvm_cov_subcommand_survives_its_own_options(self):
            # Same shapes as the nextest walk, and the same reason. llvm-cov
            # writes `[SUBCOMMAND] [OPTIONS]`, so a leading option must not hide
            # the subcommand behind it.
            self.assertEqual(2, llvm_cov_subcommand_index(["cargo", "llvm-cov", "report", "--json"]))
            self.assertEqual(
                3, llvm_cov_subcommand_index(["cargo", "llvm-cov", "--verbose", "show-env"])
            )
            # An option this file has never heard of, whose value is skipped
            # because it is not a subcommand.
            self.assertEqual(
                4, llvm_cov_subcommand_index(["cargo", "llvm-cov", "--new-upstream-option", "VALUE", "clean"])
            )
            self.assertIsNone(llvm_cov_subcommand_index(["cargo", "llvm-cov", "--", "clean"]))
            self.assertIsNone(llvm_cov_subcommand_index(["cargo", "llvm-cov"]))
            # An option's value may be spelled like a subcommand, and there is no
            # way to tell that from a real one. The first match wins.
            self.assertEqual(3, llvm_cov_subcommand_index(["cargo", "llvm-cov", "--manifest-path", "clean", "run"]))

        def test_the_two_lists_of_llvm_cov_subcommands_are_one_list(self):
            # The same rule the nextest pair holds to: a subcommand in one list
            # and not the other is a command published under a kind that says it
            # is not one.
            self.assertTrue(LLVM_COV_REPORTING_SUBCOMMANDS)
            self.assertTrue(LLVM_COV_REPORTING_SUBCOMMANDS <= LLVM_COV_KNOWN_SUBCOMMANDS)
            for sub in sorted(LLVM_COV_REPORTING_SUBCOMMANDS):
                self.assertEqual("coverage", command_kind(["cargo", "llvm-cov", sub]), sub)
            for sub in sorted(LLVM_COV_KNOWN_SUBCOMMANDS - LLVM_COV_REPORTING_SUBCOMMANDS):
                self.assertEqual(LLVM_COV_LOOKUP_KIND, command_kind(["cargo", "llvm-cov", sub]), sub)

        def test_the_known_llvm_cov_subcommands_match_the_installed_tool(self):
            # Read off the tool's own help, for the reason the nextest check
            # gives: an unrecognised subcommand is classified as a probe, which
            # is the safe side but silently publishes a coverage run under the
            # wrong name, so an upstream addition has to fail a check rather than
            # change behaviour. GATE_ACTIVE is what the shim looks for to pass
            # straight through, so asking the question cannot itself be cached.
            proc = subprocess.run(
                ["cargo", "llvm-cov", "--help"],
                capture_output=True,
                text=True,
                check=False,
                env=dict(os.environ, GATE_ACTIVE="1"),
            )
            if proc.returncode != 0 or "SUBCOMMANDS:" not in proc.stdout:
                self.skipTest("cargo llvm-cov is not installed on this machine")
            plain = ANSI_ESCAPE.sub("", proc.stdout)
            body = plain.partition("SUBCOMMANDS:")[2].lstrip("\n")
            listed: set[str] = set()
            for line in body.splitlines():
                if not line.strip():
                    break
                # Every subcommand in this tool's own help carries a
                # description on the following lines, so the run of indented
                # description text is skipped until the next bare name.
                if line.startswith("        ") or line.startswith("\t"):
                    continue
                parts = line.split()
                if not parts:
                    continue
                listed.add(parts[0])
            self.assertTrue(listed, "the help output was parsed into nothing")
            self.assertEqual(
                sorted(listed),
                sorted(LLVM_COV_KNOWN_SUBCOMMANDS),
                "the llvm-cov subcommands this file recognises no longer match the installed tool. "
                "Anything new is classified as a probe rather than a coverage run, so add it to "
                "LLVM_COV_KNOWN_SUBCOMMANDS, and to LLVM_COV_REPORTING_SUBCOMMANDS if it produces a report.",
            )
            self.assertTrue(
                LLVM_COV_REPORTING_SUBCOMMANDS <= listed,
                "this file claims a subcommand produces a report that the tool does not offer",
            )

        def test_a_make_dry_run_is_never_gated(self):
            # The second defect. The shim's loop skipped every argument starting
            # with `-`, so the dry-run flag never stopped it: `make -n coverage`
            # was classified heavy, and the recipe list it printed was published
            # as the coverage verdict under the coverage kind's name, replacing
            # the log of the run it stood in for. Every spelling GNU make accepts
            # has to pass through.
            shim = Path(__file__).resolve().parent / "gate-shim"
            # Driven rather than parsed. The shim is a shell script and the
            # defect was in its loop, so a check that only reads the file would
            # pass against a shim carrying the right words and the same bug. The
            # shim is installed as a symlink named `make` earlier on PATH, with
            # a stand-in behind it, exactly as `gate-install.sh` does it: the
            # stand-in records whether the gate wrapped it, and the gate exports
            # GATE_ACTIVE to the command it runs, so a direct pass-through leaves
            # no mark.
            with exec_tempdir(purpose="a stand-in make command") as tmp:
                shimdir = Path(tmp) / "shim"
                realdir = Path(tmp) / "real"
                shimdir.mkdir()
                realdir.mkdir()
                marker = Path(tmp) / "gated"
                stand_in = realdir / "make"
                stand_in.write_text(
                    f'#!/bin/sh\n'
                    f'if [ "${{GATE_ACTIVE:-}}" = 1 ]; then echo gated >> "{marker}"; fi\n'
                    f'exit 0\n'
                )
                stand_in.chmod(0o755)
                (shimdir / "make").symlink_to(shim)
                env = dict(
                    os.environ,
                    PATH=f"{shimdir}{os.pathsep}{realdir}{os.pathsep}{os.environ.get('PATH', '')}",
                    GATE_ACTIVE="",
                )
                # The repo root is the working directory on purpose: the shim
                # asks git for the checkout before it classifies anything, so a
                # run from anywhere else would pass through for that reason
                # instead and every row below would be measuring the wrong thing.
                root = Path(__file__).resolve().parent.parent
                for row, (args, makeflags, expect_gated) in enumerate(
                    (
                        # Every spelling of the dry-run flag as an argument.
                        (["-n", "coverage"], "", False),
                        (["--dry-run", "coverage"], "", False),
                        (["--just-print", "coverage"], "", False),
                        (["--recon", "coverage"], "", False),
                        (["-ns", "coverage"], "", False),
                        (["-C", ".", "-n", "coverage"], "", False),
                        (["coverage", "-n"], "", False),
                        # And through MAKEFLAGS, which GNU make also reads.
                        (["coverage"], "-n", False),
                        (["coverage"], "--dry-run", False),
                        # Real options carrying an `n`, which must stay gated.
                        # A check that simply refused anything with an `n` in it
                        # would pass the rows above and fail these.
                        (["--no-print-directory", "coverage"], "", True),
                        (["-e", "coverage"], "", True),
                        # The same, arriving as an export rather than an argument.
                        (["coverage"], "--no-print-directory", True),
                        (["coverage"], "--output-sync=target", True),
                        (["coverage"], "--keep-going", True),
                        (["coverage"], "-j4", True),
                        # The plain call, so a check that gated nothing at all
                        # fails here rather than passing every row above.
                        (["coverage"], "", True),
                    )
                ):
                    before = marker.read_text() if marker.is_file() else ""
                    # A private cache per row, because the gate's whole point is
                    # that a repeat on an unchanged tree replays instead of
                    # running. Sharing one cache made every gated row after the
                    # first replay, so the stand-in was never reached and the row
                    # failed for a reason that had nothing to do with the flag.
                    cache = Path(tmp) / f"cache-{row}"
                    call_env = dict(env, MAKEFLAGS=makeflags, XDG_CACHE_HOME=str(cache))
                    proc = subprocess.run(
                        [str(shimdir / "make"), *args],
                        capture_output=True,
                        text=True,
                        check=False,
                        env=call_env,
                        cwd=root,
                    )
                    after = marker.read_text() if marker.is_file() else ""
                    self.assertEqual(
                        expect_gated,
                        len(after.splitlines()) > len(before.splitlines()),
                        f"{args} MAKEFLAGS={makeflags!r}: the shim "
                        f"{'gated' if expect_gated else 'passed through'} nothing",
                    )
                    if not expect_gated:
                        self.assertEqual(0, proc.returncode, args)
                    shutil.rmtree(cache, ignore_errors=True)

        def test_a_clean_never_replays(self):
            # `clean` removes build output, so a replayed verdict would report
            # success without having removed anything. Both spellings are named
            # as a command rather than as a target, because that is how the tool
            # receives it.
            for argv in (
                ["make", "coverage-clean"],
                ["cargo", "llvm-cov", "clean"],
                ["cargo", "llvm-cov", "clean", "--workspace"],
            ):
                self.assertNotEqual("coverage", command_kind(argv))
            with exec_tempdir(purpose="a stand-in cargo command") as tmp:
                counter = Path(tmp) / "count"
                bindir = Path(tmp) / "bin"
                bindir.mkdir()
                stand_in = bindir / "cargo"
                stand_in.write_text(f'#!/bin/sh\necho run >> "{counter}"\nexit 5\n')
                stand_in.chmod(0o755)
                clean = [str(stand_in), "llvm-cov", "clean"]
                env_patch = mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(Path(tmp) / "cache")})
                root_patch = mock.patch("__main__.repo_root", return_value="/repo")
                tree_patch = mock.patch(
                    "__main__.read_tree_state",
                    return_value=state("h1", diff="clean", changed=("crates/a/src/lib.rs",)),
                )
                quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
                with env_patch, root_patch, tree_patch, quiet[0], quiet[1]:
                    run_gate(clean)
                    run_gate(clean)
                    self.assertEqual(2, counter.read_text().count("run"))

        def test_the_two_lists_of_nextest_subcommands_are_one_list(self):
            # Narrowing a command and classifying it are two decisions about the
            # same fact, and they used to be written down twice. A subcommand in
            # one list and not the other is a command cached as a test but never
            # narrowed, or narrowed under a kind that says it is not one.
            self.assertTrue(NEXTEST_EXECUTING_SUBCOMMANDS)
            self.assertTrue(NEXTEST_EXECUTING_SUBCOMMANDS <= NEXTEST_KNOWN_SUBCOMMANDS)
            for sub in sorted(NEXTEST_EXECUTING_SUBCOMMANDS):
                self.assertEqual("test", command_kind(["cargo", "nextest", sub]))
                with mock.patch("__main__.workspace_members", return_value=members):
                    scoped, _ = scope_command(
                        "test", ["cargo", "nextest", sub, "--workspace"], "/repo", ("crates/b/src/lib.rs",)
                    )
                self.assertIn("-p", scoped, f"{sub} runs tests and should be narrowed")
            for sub in sorted(NEXTEST_KNOWN_SUBCOMMANDS - NEXTEST_EXECUTING_SUBCOMMANDS):
                self.assertEqual(NEXTEST_LOOKUP_KIND, command_kind(["cargo", "nextest", sub]), sub)
                self.assertEqual(
                    (["cargo", "nextest", sub, "--workspace"], None),
                    scope_command(
                        NEXTEST_LOOKUP_KIND,
                        ["cargo", "nextest", sub, "--workspace"],
                        "/repo",
                        ("crates/b/src/lib.rs",),
                    ),
                    sub,
                )

        def test_the_package_flag_lands_after_an_option_led_subcommand(self):
            # The bug this shape caused: a fixed index put the package flag in
            # front of the subcommand, and `cargo nextest -p clio list` is not a
            # legal command, so the hook that runs the listing reported a
            # failure carrying an error from a command nobody had asked for.
            with mock.patch("__main__.workspace_members", return_value=members):
                scoped, scope = scope_command(
                    "test", ["cargo", "nextest", "--verbose", "run", "--workspace"], "/repo", ("crates/b/src/lib.rs",)
                )
            self.assertEqual(["cargo", "nextest", "--verbose", "run", "-p", "a", "-p", "b"], scoped)
            self.assertEqual(["a", "b"], scope)
            with mock.patch("__main__.workspace_members", return_value=members):
                aliased, _ = scope_command(
                    "test", ["cargo", "nextest", "-P", "ci", "r", "--workspace"], "/repo", ("crates/b/src/lib.rs",)
                )
            self.assertEqual(["cargo", "nextest", "-P", "ci", "r", "-p", "a", "-p", "b"], aliased)

        def test_every_coverage_kind_is_registered_as_one(self):
            # A kind that runs the coverage report but is missing from this set
            # would not save its artifact and would survive a clean, which is
            # the report the next run compares against.
            for target, kind in sorted(MAKE_KINDS.items()):
                if "coverage" in target or "cov" == target:
                    self.assertIn(kind, COVERAGE_KINDS, f"{target} publishes as {kind}, which is not registered")
            self.assertIn("coverage", COVERAGE_KINDS)
            self.assertIn("coverage-fast", COVERAGE_KINDS)
            # A clean is recognised by the target it was asked for, because the
            # kind no longer says "coverage".
            self.assertEqual("coverage-clean", command_kind(["make", "coverage-clean"]))
            self.assertEqual("coverage-html", command_kind(["make", "coverage-html"]))
            self.assertEqual("coverage", command_kind(["make", "coverage"]))
            self.assertEqual("coverage", command_kind(["make", "cov"]))
            for derived in ("coverage-html", "coverage-lcov", "coverage-open", "coverage-clean"):
                self.assertNotEqual(
                    "coverage", command_kind(["make", derived]), f"{derived} would overwrite the coverage log"
                )
            # Coverage is never narrowed, whatever its kind.
            for kind in sorted(COVERAGE_KINDS):
                argv = ["make", kind]
                self.assertEqual((argv, None), scope_command(kind, argv, "/repo", ("crates/b/src/lib.rs",)))

        def test_a_coverage_kind_saves_one_named_path(self):
            # What the comment above COVERAGE_KINDS has to describe, checked here
            # rather than left as a sentence a reader has to take on trust. A make
            # target's argument vector never carries `--output-path`: the recipe
            # that would is inside the Makefile, and this wrapper is handed the
            # target name. So every coverage kind falls back to the single path
            # named below, and only `make coverage` writes it. A kind that does not
            # write that path is registered for the verdict, not for an artifact.
            for target in sorted(MAKE_KINDS):
                if MAKE_KINDS[target] not in COVERAGE_KINDS:
                    continue
                self.assertEqual(
                    [COVERAGE_ARTIFACT],
                    _artifact_rel_paths(["make", target], MAKE_KINDS[target]),
                    f"{target} saves an artifact other than the one named here",
                )
            # The half that does work: a command that names its own output path.
            # A tool called directly gets that path read out of the command line.
            self.assertEqual(
                ["reports/out.json"],
                _artifact_rel_paths(["cargo", "llvm-cov", "report", "--output-path", "reports/out.json"], "report"),
            )

        def test_a_replay_puts_back_the_artifact_it_saved(self):
            with tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp) / "repo"
                entry = Path(tmp) / "entry"
                report = Path(root) / COVERAGE_ARTIFACT
                report.parent.mkdir(parents=True)
                report.write_bytes(b'{"run": "first"}')
                saved = _save_artifacts(entry, str(root), ["make", "coverage"], "coverage")
                self.assertEqual([COVERAGE_ARTIFACT], saved)
                report.unlink()
                with contextlib.redirect_stdout(io.StringIO()):
                    _restore_artifacts(entry, str(root), {"artifacts": saved})
                self.assertEqual(b'{"run": "first"}', report.read_bytes())

        def test_a_replay_leaves_an_artifact_written_since_it_alone(self):
            # A report is under `target/`, so it is not in the tree fingerprint and
            # a replay can meet one that is newer than the copy it holds. Writing
            # the older copy back would leave the reader with a measurement of a
            # tree that is not this one, and nothing in the log to say so. The
            # fingerprint names the source, never the build output.
            with tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp) / "repo"
                entry = Path(tmp) / "entry"
                report = Path(root) / COVERAGE_ARTIFACT
                report.parent.mkdir(parents=True)
                report.write_bytes(b'{"run": "cached"}')
                saved = _save_artifacts(entry, str(root), ["make", "coverage"], "coverage")
                report.write_bytes(b'{"run": "newer"}')
                with contextlib.redirect_stdout(io.StringIO()):
                    _restore_artifacts(entry, str(root), {"artifacts": saved})
                self.assertEqual(b'{"run": "newer"}', report.read_bytes())

        def test_a_replay_replaces_an_emptied_artifact(self):
            # An empty file is what a shell redirect or an interrupted write leaves
            # behind, and it holds no measurement, so restoring over it is a repair
            # rather than a loss.
            with tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp) / "repo"
                entry = Path(tmp) / "entry"
                report = Path(root) / COVERAGE_ARTIFACT
                report.parent.mkdir(parents=True)
                report.write_bytes(b'{"run": "cached"}')
                saved = _save_artifacts(entry, str(root), ["make", "coverage"], "coverage")
                report.write_bytes(b"")
                with contextlib.redirect_stdout(io.StringIO()):
                    _restore_artifacts(entry, str(root), {"artifacts": saved})
                self.assertEqual(b'{"run": "cached"}', report.read_bytes())

        def test_changed_seeds_and_root_file(self):
            self.assertEqual({"b"}, changed_seeds("/repo", ("crates/b/src/lib.rs",), members))
            # A root file that no crate compiles against is ignored for scoping,
            # so the seed set stays empty instead of forcing a workspace-wide run.
            self.assertEqual(set(), changed_seeds("/repo", ("README.md",), members))
            # A root file that does reach every crate is still workspace-wide.
            self.assertIsNone(changed_seeds("/repo", ("Cargo.toml",), members))
            self.assertEqual(set(), changed_seeds("/repo", (), members))

        def test_closure_follows_reverse_dependencies(self):
            self.assertEqual(["a", "b"], closure({"b"}, members))
            self.assertEqual(["c"], closure({"c"}, members))

        def test_scope_command_rewrites_workspace(self):
            with mock.patch("__main__.workspace_members", return_value=members):
                scoped, scope = scope_command("test", ["cargo", "test", "--locked", "--workspace"], "/repo", ("crates/b/src/lib.rs",))
            self.assertEqual(["cargo", "test", "-p", "a", "-p", "b", "--locked"], scoped)
            self.assertEqual(["a", "b"], scope)

        def test_scope_command_keeps_flags_before_double_dash(self):
            with mock.patch("__main__.workspace_members", return_value=members):
                scoped, _ = scope_command("test", ["cargo", "test", "--locked", "--", "--nocapture"], "/repo", ("crates/b/src/lib.rs",))
            self.assertEqual(["cargo", "test", "-p", "a", "-p", "b", "--locked", "--", "--nocapture"], scoped)
            with mock.patch("__main__.workspace_members", return_value=members):
                scoped, _ = scope_command("test", ["cargo", "nextest", "run", "--workspace"], "/repo", ("crates/b/src/lib.rs",))
            self.assertEqual(["cargo", "nextest", "run", "-p", "a", "-p", "b"], scoped)

        def test_a_lint_run_that_named_the_workspace_is_not_narrowed(self):
            # The command the pull request template prints, run by a contributor
            # on a machine where the shim is installed and the tree is dirty. The
            # wrapper used to strip `--workspace` and substitute the changed
            # crates, so a run the checklist described as workspace-wide measured
            # a handful of crates and published the result as the lint verdict.
            # `make lint` stays scoped: that is a different path, through
            # PACKAGES, and it is scoped on purpose.
            command = ["cargo", "clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"]
            with mock.patch("__main__.workspace_members", return_value=members):
                scoped, scope = scope_command("clippy", command, "/repo", ("crates/b/src/lib.rs",))
            self.assertEqual(command, scoped)
            self.assertIsNone(scope)

        def test_an_unstated_lint_run_is_still_narrowed(self):
            # The exemption is for a run that stated its scope. One that did not
            # is narrowed as before, or the fast local lint disappears with it.
            with mock.patch("__main__.workspace_members", return_value=members):
                scoped, scope = scope_command("clippy", ["cargo", "clippy", "--all-targets"], "/repo", ("crates/b/src/lib.rs",))
            self.assertEqual(["cargo", "clippy", "-p", "a", "-p", "b", "--all-targets"], scoped)
            self.assertEqual(["a", "b"], scope)

        def test_a_lint_run_that_named_one_crate_is_still_left_alone(self):
            command = ["cargo", "clippy", "-p", "b", "--all-targets"]
            with mock.patch("__main__.workspace_members", return_value=members):
                scoped, scope = scope_command("clippy", command, "/repo", ("crates/b/src/lib.rs",))
            self.assertEqual(command, scoped)
            self.assertIsNone(scope)

        def test_scope_leaves_unmodelled_nextest_subcommands_alone(self):
            # The exact command the pre-commit hook runs. Narrowing it broke the
            # hook, because a package flag before the subcommand is not a legal
            # command at all and nextest refused to run any of it.
            #
            # The kind comes from the classifier rather than a literal, so this
            # keeps testing what production does with the command instead of
            # what it did when the two were written down by hand.
            argv = ["cargo", "nextest", "list", "--workspace", "--locked"]
            self.assertEqual(NEXTEST_LOOKUP_KIND, command_kind(argv))
            self.assertEqual(
                (argv, None), scope_command(command_kind(argv), argv, "/repo", ("crates/b/src/lib.rs",))
            )
            for sub in ("show-config", "archive", "bench", "self", "help"):
                other = ["cargo", "nextest", sub, "--workspace"]
                self.assertEqual(
                    (other, None), scope_command(command_kind(other), other, "/repo", ("crates/b/src/lib.rs",))
                )

        def test_scope_places_the_flag_after_the_nextest_subcommand(self):
            with mock.patch("__main__.workspace_members", return_value=members):
                scoped, _ = scope_command("test", ["cargo", "nextest", "run", "--locked", "--workspace"], "/repo", ("crates/b/src/lib.rs",))
            self.assertEqual(["cargo", "nextest", "run", "-p", "a", "-p", "b", "--locked"], scoped)
            with mock.patch("__main__.workspace_members", return_value=members):
                aliased, _ = scope_command("test", ["cargo", "nextest", "r", "--workspace"], "/repo", ("crates/b/src/lib.rs",))
            self.assertEqual(["cargo", "nextest", "r", "-p", "a", "-p", "b"], aliased)

        def test_the_cheap_local_gate_is_cached_and_never_narrowed(self):
            # `make ritual` must publish under its own kind so the procedure
            # files can read logs/ritual.log, and must NOT be narrowed: its clippy
            # is workspace-wide on purpose, because a crate the change did not
            # touch can still fail to compile against a signature it changed.
            self.assertEqual("ritual", command_kind(["make", "ritual"]))
            with mock.patch("__main__.workspace_members", return_value=members):
                scoped, scope = scope_command("ritual", ["make", "ritual"], "/repo", ("crates/b/src/lib.rs",))
            self.assertIsNone(scope)
            self.assertEqual(["make", "ritual"], scoped)

        def test_a_renamed_gate_target_stays_in_the_shim_dispatch_list(self):
            # The wrapper's coverage of a make target lives in two places: the
            # shim's dispatch list, which decides whether the call is gated at
            # all, and MAKE_KINDS, which decides the published kind name. A target
            # missing from the first is never cached, never locked and never
            # published, which is how `verify` sat outside the wrapper for its
            # whole life while three files claimed it was inside it.
            #
            # The dispatch list is the `heavy=1` case arm inside the loop over
            # `"$@"`. Parsing it as prose is what made an earlier version of this
            # test pass against a shim that no longer listed the target, so this
            # one only accepts a target that appears on the arm that sets heavy=1.
            shim = Path(__file__).resolve().parent / "gate-shim"
            text = shim.read_text()
            lines = text.splitlines()
            heavy_at = [i for i, line in enumerate(lines) if "heavy=1" in line]
            self.assertTrue(heavy_at, "the shim no longer marks any command heavy")
            gated: set[str] = set()
            for i in heavy_at:
                # A `case` arm is `pattern...)` and its body follows, so the
                # patterns are on the line before the `heavy=1` body, not on it.
                if i == 0:
                    continue
                for token in lines[i - 1].split("|"):
                    name = token.strip().rstrip(")").strip()
                    if name:
                        gated.add(name)
            missing = sorted(set(MAKE_KINDS) - gated)
            self.assertEqual(
                [],
                missing,
                f"in MAKE_KINDS but not gated by the shim: {missing}. "
                "Such a target is never cached, never locked and never published.",
            )

        def test_scope_respects_explicit_package_and_coverage(self):
            argv = ["cargo", "test", "-p", "c"]
            self.assertEqual((argv, None), scope_command("test", argv, "/repo", ("crates/b/src/lib.rs",)))
            cov = ["cargo", "llvm-cov", "nextest", "--workspace"]
            self.assertEqual((cov, None), scope_command("coverage", cov, "/repo", ("crates/b/src/lib.rs",)))

        def test_scope_make_targets_through_packages(self):
            with mock.patch("__main__.workspace_members", return_value=members):
                scoped, scope = scope_command("check", ["make", "check"], "/repo", ("crates/b/src/lib.rs",))
            self.assertEqual(["make", "check", "PACKAGES=-p a -p b"], scoped)
            self.assertEqual(["a", "b"], scope)
            cov = ["make", "coverage"]
            self.assertEqual((cov, None), scope_command("coverage", cov, "/repo", ("crates/b/src/lib.rs",)))
            self.assertEqual((cov, None), scope_command("coverage", cov, "/repo", ("Cargo.toml",)))
            explicit = ["make", "test", "PACKAGES=-p c"]
            self.assertEqual((explicit, None), scope_command("test", explicit, "/repo", ("crates/b/src/lib.rs",)))
            with mock.patch.dict(os.environ, {"PACKAGES": "-p c"}):
                self.assertEqual(
                    (["make", "test"], None),
                    scope_command("test", ["make", "test"], "/repo", ("crates/b/src/lib.rs",)),
                )

        def test_command_kind(self):
            self.assertEqual("test", command_kind(["cargo", "nextest", "run"]))
            self.assertEqual("coverage", command_kind(["cargo", "llvm-cov", "nextest"]))
            self.assertEqual("check", command_kind(["make", "-C", "x", "check"]))
            self.assertEqual("coverage", command_kind(["make", "PROFILE=", "coverage"]))
            self.assertEqual("coverage-clean", command_kind(["make", "coverage-clean"]))

        def test_purge_removes_only_coverage_entries(self):
            with tempfile.TemporaryDirectory() as tmp:
                with mock.patch.dict(os.environ, {"XDG_CACHE_HOME": tmp}):
                    repo = cache_root() / repo_key("/repo")
                    for name, kind in (("a", "coverage"), ("b", "test")):
                        entry = repo / name
                        entry.mkdir(parents=True)
                        (entry / "meta.json").write_text(json.dumps({"kind": kind}))
                    with contextlib.redirect_stdout(io.StringIO()):
                        _purge_coverage_cache("/repo")
                    self.assertFalse((repo / "a").exists())
                    self.assertTrue((repo / "b" / "meta.json").is_file())

    buffer = io.StringIO()
    result = unittest.TextTestRunner(stream=buffer, verbosity=1).run(
        unittest.defaultTestLoader.loadTestsFromTestCase(Tests)
    )

    checks: list[tuple[str, bool]] = []
    direct = state("h1", changed=("crates/a/src/lib.rs",))
    with tempfile.TemporaryDirectory() as tmp:
        counter = Path(tmp) / "count"
        cache = Path(tmp) / "cache"
        cmd = ["/bin/sh", "-c", f'echo run >> "{counter}"']
        red = ["/bin/sh", "-c", f'echo run >> "{counter}"; exit 7']
        env_patch = mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(cache)})
        root_patch = mock.patch("__main__.repo_root", return_value="/repo")
        quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
        with env_patch, root_patch, mock.patch("__main__.read_tree_state", return_value=direct), quiet[0], quiet[1]:
            checks.append(("first run executes and passes", run_gate(cmd) == 0))
            checks.append(("same tree reuses the green verdict", run_gate(cmd) == 0 and counter.read_text().count("run") == 1))
            checks.append(("red verdict returned", run_gate(red) == 7))
            checks.append(("red verdict replayed, not rerolled", run_gate(red) == 7 and counter.read_text().count("run") == 2))
            checks.append(("--force re-executes", run_gate(cmd, force=True) == 0 and counter.read_text().count("run") == 3))
        with env_patch, root_patch, mock.patch(
            "__main__.read_tree_state",
            side_effect=[state("h1", diff="one", changed=("crates/a/src/lib.rs",)), state("h1", diff="two", changed=("crates/a/src/lib.rs",))],
        ), quiet[0], quiet[1]:
            run_gate(cmd)
        with env_patch, root_patch, mock.patch("__main__.read_tree_state", return_value=state("h1", diff="one", changed=("crates/a/src/lib.rs",))), quiet[0], quiet[1]:
            run_gate(cmd)
            checks.append(("a mid-run change is not cached", counter.read_text().count("run") == 5))

    with tempfile.TemporaryDirectory() as tmp:
        counter = Path(tmp) / "count"
        cache = Path(tmp) / "cache"
        red = ["/bin/sh", "-c", f'echo run >> "{counter}"; exit 7']
        env_patch = mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(cache)})
        root_patch = mock.patch("__main__.repo_root", return_value="/repo")
        tree_patch = mock.patch(
            "__main__.read_tree_state",
            return_value=state("h1", diff="path", changed=("crates/a/src/lib.rs",)),
        )
        quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
        with env_patch, root_patch, tree_patch, mock.patch.dict(os.environ, {"PATH": "/one"}), quiet[0], quiet[1]:
            run_gate(red)
            checks.append(("red stored under a PATH", counter.read_text().count("run") == 1))
        with env_patch, root_patch, tree_patch, mock.patch.dict(os.environ, {"PATH": "/two"}), quiet[0], quiet[1]:
            checks.append(("red re-runs under a different PATH", run_gate(red) == 7 and counter.read_text().count("run") == 2))
        with env_patch, root_patch, tree_patch, mock.patch.dict(os.environ, {"PATH": "/two"}), quiet[0], quiet[1]:
            checks.append(("red replays under the same PATH", run_gate(red) == 7 and counter.read_text().count("run") == 2))

    # The two commands the wrapper used to treat as one, driven end to end.
    #
    # A stand-in executable named `cargo` is used instead of the real one, because
    # the classification reads the file name and nothing else about the command, so
    # the wrapper's decision can be exercised without a build. Each run appends to a
    # counter and prints the arguments it was handed, so the stored logs can be told
    # apart as well as the runs.
    def check_cargo_stand_in() -> None:
        with exec_tempdir(purpose="a stand-in cargo command") as tmp:
            counter = Path(tmp) / "count"
            bindir = Path(tmp) / "bin"
            bindir.mkdir()
            stand_in = bindir / "cargo"
            stand_in.write_text(f'#!/bin/sh\necho run >> "{counter}"\necho "invoked: $*"\nexit 7\n')
            stand_in.chmod(0o755)
            log_dir = Path(tmp) / "logs"
            run = [str(stand_in), "nextest", "run", "-p", "a"]
            lookup = [str(stand_in), "nextest", "list", "--workspace", "--locked"]
            env_patch = mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(Path(tmp) / "cache")})
            root_patch = mock.patch("__main__.repo_root", return_value="/repo")
            tree_patch = mock.patch(
                "__main__.read_tree_state",
                return_value=state("h1", diff="cmd", changed=("crates/a/src/lib.rs",)),
            )
            quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
            with env_patch, root_patch, tree_patch, quiet[0], quiet[1]:
                checks.append(("a test run stores its red verdict", run_gate(run, log_dir=str(log_dir)) == 7))
                checks.append(
                    ("the same run replays it", run_gate(run, log_dir=str(log_dir)) == 7 and counter.read_text().count("run") == 1)
                )
                checks.append(
                    (
                        "a test lookup runs instead of replaying the run",
                        run_gate(lookup, log_dir=str(log_dir)) == 7 and counter.read_text().count("run") == 2,
                    )
                )
                checks.append(
                    ("the lookup replays itself", run_gate(lookup, log_dir=str(log_dir)) == 7 and counter.read_text().count("run") == 2)
                )
                checks.append(
                    ("the run still replays afterwards", run_gate(run, log_dir=str(log_dir)) == 7 and counter.read_text().count("run") == 2)
                )
            lookup_log = published(f"{NEXTEST_LOOKUP_KIND}.log", log_dir)
            run_log = published("test.log", log_dir)
            checks.append(("the lookup publishes under its own kind", "nextest list" in lookup_log))
            checks.append(
                (
                    "the run's verdict is not the lookup's",
                    "nextest run" in run_log and "nextest list" not in run_log,
                )
            )
            checks.append(
                (
                    "each publishes its own exit code",
                    published(f"{NEXTEST_LOOKUP_KIND}.exit", log_dir).strip() == "7"
                    and published("test.exit", log_dir).strip() == "7",
                )
            )

    # Two `make test` runs that differ only in the package list the environment
    # supplies. The scoping code reads it, so the second run really is a different
    # command, and a key that ignored it handed the second run the first run's
    # verdict while publishing a log naming the first package list.
    def check_make_stand_in() -> None:
        with exec_tempdir(purpose="a stand-in make command") as tmp:
            counter = Path(tmp) / "count"
            bindir = Path(tmp) / "bin"
            bindir.mkdir()
            stand_in = bindir / "make"
            stand_in.write_text(f'#!/bin/sh\necho run >> "{counter}"\necho "PACKAGES=$PACKAGES"\nexit 3\n')
            stand_in.chmod(0o755)
            cmd = [str(stand_in), "test"]
            log_dir = Path(tmp) / "logs"
            env_patch = mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(Path(tmp) / "cache")})
            root_patch = mock.patch("__main__.repo_root", return_value="/repo")
            tree_patch = mock.patch(
                "__main__.read_tree_state",
                return_value=state("h1", diff="pkg", changed=("crates/a/src/lib.rs",)),
            )
            quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
            with env_patch, root_patch, tree_patch, quiet[0], quiet[1]:
                with mock.patch.dict(os.environ, {"PACKAGES": "-p a"}):
                    first = run_gate(cmd, log_dir=str(log_dir))
                with mock.patch.dict(os.environ, {"PACKAGES": "-p b"}):
                    second = run_gate(cmd, log_dir=str(log_dir))
                checks.append(
                    (
                        "a package list from the environment separates cache entries",
                        first == 3 and second == 3 and counter.read_text().count("run") == 2,
                    )
                )
            checks.append(
                (
                    "the published log names the package list that ran",
                    published("test.log", log_dir).strip() == "PACKAGES=-p b",
                )
            )

    # The log a run replaced is the evidence that the run before it happened, and
    # overwriting it means a reproduction cannot be read back once the fix lands.
    # The stand-in changes its own output and exit code on each run, and the tree
    # moves between them so the second run is a fresh one rather than a replay,
    # which is what an agent editing code does.
    #
    # This is a check of its own rather than three more assertions on the make
    # stand-in above, which already runs one command twice over one log
    # directory. Folding them together would couple keeping the replaced log to
    # telling two package lists apart, and a failure would then be reported under
    # whichever name the reader happened to be looking for.
    def check_publish_keeps_the_replaced_log() -> None:
        with exec_tempdir(purpose="a stand-in command that changes between runs") as tmp:
            counter = Path(tmp) / "count"
            bindir = Path(tmp) / "bin"
            bindir.mkdir()
            stand_in = bindir / "changing"
            stand_in.write_text(
                f'#!/bin/sh\n'
                f'n=$(cat "{counter}" 2>/dev/null || echo 0)\n'
                f'n=$((n+1))\n'
                f'echo "$n" > "{counter}"\n'
                f'if [ "$n" -eq 1 ]; then echo "before the fix"; exit 101; fi\n'
                f'echo "after the fix"; exit 0\n'
            )
            stand_in.chmod(0o755)
            cmd = [str(stand_in)]
            log_dir = Path(tmp) / "logs"
            head = {"id": "h1"}
            env_patch = mock.patch.dict(os.environ, {"XDG_CACHE_HOME": str(Path(tmp) / "cache")})
            root_patch = mock.patch("__main__.repo_root", return_value="/repo")
            tree_patch = mock.patch(
                "__main__.read_tree_state",
                side_effect=lambda *_: state(head["id"], changed=("crates/a/src/lib.rs",)),
            )
            quiet = (contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()))
            with env_patch, root_patch, tree_patch, quiet[0], quiet[1]:
                first = run_gate(cmd, log_dir=str(log_dir))
                head["id"] = "h2"
                second = run_gate(cmd, log_dir=str(log_dir))
                # Read what was published here, before the next run, so each check
                # below reports the state its own name describes rather than the
                # last state the directory happened to reach.
                kept = published("command.prev", log_dir)
                newest = published("command.log", log_dir)
                newest_code = published("command.exit", log_dir)
                replay = run_gate(cmd, log_dir=str(log_dir))
                # A shell redirect truncates the published log before the command
                # starts, which is how the empty file gets there. It must not cost
                # the run already kept.
                (log_dir / "command.log").write_text("")
                head["id"] = "h3"
                after_empty = run_gate(cmd, log_dir=str(log_dir))
                kept_after_empty = published("command.prev", log_dir)
            checks.append(
                (
                    "the run a later run replaced is still readable",
                    first == 101 and second == 0
                    and kept.splitlines()[-1:] == ["before the fix"]
                    and newest.strip() == "after the fix",
                )
            )
            checks.append(
                (
                    "the kept run names the exit code it exited with",
                    kept.startswith("exit: 101\n") and newest_code.strip() == "0",
                )
            )
            checks.append(
                (
                    "replaying a verdict does not invent an earlier run",
                    replay == 0
                    and published("command.prev", log_dir).startswith("exit: 101\n")
                    and published("command.log", log_dir).strip() == "after the fix",
                )
            )
            checks.append(
                (
                    "an emptied published log does not cost the kept run",
                    after_empty == 0 and kept_after_empty == kept,
                )
            )

    # A surface that names the log this gate publishes but not the run it keeps
    # describes half the contract, and the half it leaves out is the half that
    # preserves a reproduction. That has drifted twice: once when the gate grew
    # the kept run, and once in the rules written beside it. This reads the tree
    # rather than a list, because drift arrives as a file nobody remembered to
    # update.
    PUBLISHED_NAME = re.compile(r"<kind>\.log|CLIO_GATE_LOG_DIR")
    KEPT_NAME = re.compile(r"<kind>\.prev")
    TEXT_SUFFIXES = (".md", ".py", ".sh", ".yml", ".yaml", ".toml", ".txt", ".json")

    def check_every_surface_names_the_kept_run_too() -> None:
        # Tracked files only. A checkout carries surfaces this repository does not
        # own -- a link to another checkout's instructions, a machine-local hook --
        # and reading those would report a half-described contract in a file the
        # next person cannot edit from here. Git is also the honest answer to
        # "which files are ours", rather than a list of directories to exclude.
        root = Path(__file__).resolve().parent.parent
        try:
            listed = subprocess.run(
                ["git", "ls-files", "-z", "--", *(f"*{suffix}" for suffix in TEXT_SUFFIXES)],
                cwd=root,
                capture_output=True,
                text=True,
                check=True,
            ).stdout
        except (OSError, subprocess.CalledProcessError) as error:
            checks.append(("every surface naming the published log also names the kept run", False))
            print(f"self-test note: cannot list tracked files: {error}")
            return
        half_described: list[str] = []
        for name in listed.split("\0"):
            if not name:
                continue
            path = root / name
            try:
                text = path.read_text(errors="replace")
            except OSError:
                continue
            if PUBLISHED_NAME.search(text) and not KEPT_NAME.search(text):
                half_described.append(name)
        checks.append(
            (
                "every surface naming the published log also names the kept run",
                not half_described,
            )
        )
        for offender in sorted(half_described):
            print(f"self-test note: describes publishing but not the kept run: {offender}")

    check_every_surface_names_the_kept_run_too()

    # Both need a directory the host will run an executable file from. One whose
    # mount refuses is a correct mount with a correct file mode, and the helper
    # finds another or names the reason. That arrives here as a skip, which is
    # recorded by name: raised from inside a check it would leave the self-test
    # with a traceback instead of a verdict, which is the failure being removed.
    for label, stand_in_check in (
        ("cargo stand-in", check_cargo_stand_in),
        ("make stand-in", check_make_stand_in),
        ("log publishing", check_publish_keeps_the_replaced_log),
    ):
        try:
            stand_in_check()
        except unittest.SkipTest as skip:
            skipped.append(f"{label}: {skip}")

    failed = [name for name, ok in checks if not ok]
    if not result.wasSuccessful() or failed:
        print(buffer.getvalue())
        for name in failed:
            print(f"self-test FAIL: {name}")
    for reason in skipped:
        print(f"self-test skipped: {reason}")
    clean = result.wasSuccessful() and not failed
    if not clean:
        summary = "FAILED"
    elif skipped:
        summary = f"all passed, {len(skipped)} check group(s) skipped"
    else:
        summary = "all passed"
    print(f"self-test: {summary}")
    return 0 if clean else 1


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return _self_test()
    if "-h" in argv or "--help" in argv:
        print(__doc__.strip())
        return 0
    if not argv or argv[0] != "run":
        print(USAGE, file=sys.stderr)
        return EXIT_USAGE
    rest = argv[1:]
    if "--" not in rest:
        print(USAGE, file=sys.stderr)
        return EXIT_USAGE
    sep = rest.index("--")
    head = list(rest[:sep])
    force = "--force" in head or os.environ.get("GATE_FORCE") == "1"
    log_dir = ""
    while "--log-dir" in head:
        index = head.index("--log-dir")
        if index + 1 >= len(head):
            print(USAGE, file=sys.stderr)
            return EXIT_USAGE
        log_dir = head[index + 1]
        del head[index : index + 2]
    command = rest[sep + 1 :]
    if not command:
        print(USAGE, file=sys.stderr)
        return EXIT_USAGE
    try:
        return run_gate(command, force=force, log_dir=log_dir)
    except GateError as e:
        print(f"gate: error: {e}", file=sys.stderr)
        return EXIT_INTERNAL
    except Exception as e:  # noqa: BLE001 - never fail silently; the shim falls back
        print(f"gate: internal error: {e!r}", file=sys.stderr)
        return EXIT_INTERNAL


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
