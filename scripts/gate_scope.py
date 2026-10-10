from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

from gate_classify import nextest_subcommand_index
from gate_kinds import GATE_SCOPE_DEFAULT, GATE_SCOPE_OFF, NEXTEST_EXECUTING_SUBCOMMANDS, SCOPED_KINDS
from gate_tree import Member

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
    members = getattr(sys.modules.get("gate"), "workspace_members", workspace_members)(argv[0], root)
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
    members = getattr(sys.modules.get("gate"), "workspace_members", workspace_members)("cargo", root)
    if not members:
        return argv, None
    seeds = changed_seeds(root, changed, members)
    if not seeds:
        return argv, None
    packages = closure(seeds, members)
    flags = " ".join(f"-p {pkg}" for pkg in packages)
    return [*argv, f"PACKAGES={flags}"], packages
