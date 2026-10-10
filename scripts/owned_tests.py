#!/usr/bin/env python3
"""Which tests belong to a source file, read from the repository's own declarations.

This module answers one question: change this file, which tests must run. It
answers it by parsing the module declarations the compiler already has to obey,
rather than by guessing a name from a filename.

Guessing is recorded here as a failure in this repository. Two attempts were made
and both skipped real tests while reporting success, because a module name is a
choice and not a rule: `verify_fault_tests.rs` is mounted as `fault_tests`, and
`lib.rs` mounts `batch_tests_persona.rs` as `batch_tests_persona`, where the file
name contains `tests` but not in the position a suffix test looks for. Skipping a
test is invisible, because the run still passes.

So the deciding signal here is never a name. A declaration counts as a test
module when any of three properties holds, unioned because each alone misses real
cases:

1. the declaration is gated to test builds. Gating is enforced by the compiler,
   which is the property a name is not. This is what finds `batch_tests_persona`.
2. the declaration points at a `foo*_tests.rs` file, which is the ownership
   convention. This catches declarations whose own gate is absent.
3. the file holds `#[test]` functions at its own level, so its own module is the
   test module.

Reading the source is `scripts/owned_scanner.py`. Deciding what the declarations
mean is here, because that is the part worth checking against the test runner.

Boundaries:

- This module decides what to RUN and what to attribute coverage to. It holds no
  coverage threshold and compares no numbers.
- It reads source text and does not ask the compiler, so a module produced by a
  macro or a build script is invisible. No such module exists in this workspace,
  which was checked rather than assumed.

Usage:
  scripts/owned_tests.py crates/clio-backup/src/verify.rs   # what this file owns
  scripts/owned_tests.py --crate clio-backup                # whole crate
  scripts/owned_tests.py --accept                           # check against the runner
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Iterable, NamedTuple

sys.path.insert(0, str(Path(__file__).resolve().parent))

from integration_targets import (  # noqa: E402
    modules_in_target,
    target_of,
)
from owned_scanner import (  # noqa: E402
    Declaration,
    SourceError,
    children_dir,
    declarations,
    holds_tests_text,
    read_text,
    resolve_child,
    looks_like_test_file,
    test_modules_in,
    without_inline_bodies,
)

ROOT = Path(__file__).resolve().parent.parent

class ResolutionError(Exception):
    """The module tree could not be built, so nothing can be attributed."""


def crate_roots(crate_dir: Path) -> list[Path]:
    """The entry-point files of a crate: lib.rs and every main.rs."""
    src = crate_dir / "src"
    found: list[Path] = []
    if (src / "lib.rs").exists():
        found.append(src / "lib.rs")
    found.extend(sorted(src.glob("main.rs")))
    return found


def walk_crate(crate_dir: Path) -> dict[Path, str]:
    """Map every source file in a crate to the module path the compiler gives it.

    Built by following declarations from the crate root, which is the only way to
    know a file's real module path: a file mounted as
    `#[path = "verify_fault_tests.rs"] mod fault_tests;` has the module path
    `verify::fault_tests`, and nothing in its own name says so.
    """
    roots = crate_roots(crate_dir)
    if not roots:
        raise ResolutionError(f"{crate_dir} has no lib.rs or main.rs")
    module_of: dict[Path, str] = {}
    seen: set[Path] = set()

    def visit(path: Path, module_path: str) -> None:
        resolved = path.resolve()
        if resolved in seen:
            return
        seen.add(resolved)
        module_of[path] = module_path
        source = read_text(path)
        for decl in declarations(source, path.parent):
            if decl.inline:
                # An inline block with no body of its own is a declaration whose
                # contents live in `mod.rs` beside the parent.
                child = path.parent / "mod.rs"
                target = child if child.exists() else None
            else:
                target = resolve_child(children_dir(path), decl)
            if target is None:
                continue
            child_path = f"{module_path}::{decl.name}" if module_path else decl.name
            visit(target, child_path)

    for root in roots:
        visit(root, "")
    return module_of


def owned_modules(crate_dir: Path, target: Path) -> list[str]:
    """The test modules a file owns, as module paths from the crate root.

    Empty when the file owns no tests. That emptiness is a real answer and not a
    failure: it is what a file with no test of its own looks like, and the gate
    reports it as such rather than widening the selection to hide it.
    """
    module_of = walk_crate(crate_dir)
    resolved = target.resolve()
    match = next((p for p in module_of if p.resolve() == resolved), None)
    if match is None:
        raise ResolutionError(
            f"{target} is not reachable from any crate root in {crate_dir}"
        )
    return test_modules_in(read_text(match), module_of[match], match)


def crate_test_modules(crate_dir: Path) -> dict[str, list[str]]:
    """Every module holding at least one test, grouped by owning file.

    Grouped because the acceptance check needs the attribution: a test module in
    the runner's list that no file claims is the silent skip this exists to make
    impossible.
    """
    owners: dict[str, list[str]] = {}
    for path, module_path in sorted(walk_crate(crate_dir).items(), key=lambda kv: kv[1]):
        owned = test_modules_in(read_text(path), module_path, path, [])
        if owned:
            owners[str(path.relative_to(ROOT))] = owned
    return owners


def workspace_crates() -> dict[str, Path]:
    """Map package name to crate directory, from `cargo metadata`.

    The package name is not always the directory name, and the two are not
    interchangeable: keying predictions by directory name puts a mismatched
    crate's predictions in a bucket the runner's own list never mentions, so a
    correct lookup reports every module in that crate as unclaimed.
    """
    proc = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--locked", "--format-version", "1"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
        # A cached lookup verdict carries log chatter instead of the manifest,
        # which would fail the parse below and silently skip the crate.
        env={**os.environ, "GATE_RAW": "1"},
    )
    if proc.returncode != 0:
        raise ResolutionError(f"cargo metadata failed: {proc.stderr.strip()}")
    try:
        packages = json.loads(proc.stdout)["packages"]
    except (ValueError, KeyError) as error:
        raise ResolutionError(f"cargo metadata is not usable: {error}") from error
    out: dict[str, Path] = {}
    for package in packages:
        manifest = Path(package["manifest_path"])
        if (manifest.parent / "src").is_dir():
            out[package["name"]] = manifest.parent
    return out


def crate_of(rel: str) -> Path:
    """The crate directory holding a repository-relative path."""
    target = (ROOT / rel).resolve()
    crate_dir = next(
        (d for d in workspace_crates().values() if target.is_relative_to(d)), None
    )
    if crate_dir is None:
        raise ResolutionError(f"{rel} belongs to no workspace crate")
    return crate_dir


class OwnedSelection(NamedTuple):
    """What one changed file owns: a test target, and the modules inside it."""

    file: str
    binary: str | None
    modules: list[str]

    def expression(self) -> str:
        return filter_expression(self.modules, self.binary)

    @property
    def is_empty(self) -> bool:
        return not self.modules


def owned_selection(rel: str) -> OwnedSelection:
    """What a changed file owns, whether it is a source file or a test file.

    One entry point for both shapes, because a change can touch either and the
    file scope has to answer for both. Before this, a path under `tests/` raised
    "not reachable from any crate root" and the gate exited 3, which made any
    change to an integration test unrunnable.
    """
    crate_dir = crate_of(rel)
    integration = target_of(crate_dir, rel)
    if integration is not None:
        return OwnedSelection(
            rel, integration.binary, modules_in_target(integration.entry)
        )
    return OwnedSelection(rel, None, owned_modules(crate_dir, (ROOT / rel).resolve()))


def filter_expression(modules: Iterable[str], binary: str | None = None) -> str:
    """One nextest filter expression matching any test in `modules`.

    A union in one expression, never several invocations, because a test satisfies
    an expression or it does not: a name appearing twice costs nothing, while two
    separate runs would execute the overlap twice. Measured here: an expression
    carrying a duplicated branch ran the same tests as the clean one.

    The module path is anchored at both ends. `test()` is an unanchored substring
    match, so an unanchored `archive_name::tests` would also select
    `other::archive_name::tests` in a different binary.

    `binary` is required whenever a target is known and the module path is empty,
    because every integration target's root-level tests share the empty module
    path: 21 targets in one package share it, so a name-only filter would select
    all of them. `binary()` takes the bare target name, which is not the
    `package::target` column the runner's list prints.
    """
    alternatives = sorted({m for m in modules if m})
    if not binary and not alternatives:
        return ""
    if binary:
        by_module = [
            f"test(/^{re.escape(module)}(::|$)/)" for module in alternatives
        ]
        name_part = "(" + " | ".join(by_module) + ")" if len(by_module) > 1 else (
            by_module[0] if by_module else ""
        )
        base = f"binary({binary})"
        if not name_part:
            return base
        return f"{base} and {name_part}"
    if len(alternatives) == 1:
        return f"test(/^{re.escape(alternatives[0])}(::|$)/)"
    branches = [f"test(/^{re.escape(module)}(::|$)/)" for module in alternatives]
    return "(" + " | ".join(branches) + ")"


def main(argv: list[str]) -> int:
    args = list(argv)
    try:
        if args and args[0] == "--accept":
            from owned_accept import accept_against_runner  # noqa: E402

            return accept_against_runner()
        if args and args[0] == "--crate":
            if len(args) < 2:
                print("usage: owned_tests.py --crate <name>", file=sys.stderr)
                return 2
            for path, modules in sorted(crate_test_modules(ROOT / "crates" / args[1]).items()):
                print(f"{path}  ->  {', '.join(modules)}")
            return 0
        for rel in args:
            selection = owned_selection(rel)
            where = f" in target {selection.binary}" if selection.binary else ""
            print(f"{rel}: {len(selection.modules)} test module(s){where}")
            for module in selection.modules:
                print(f"    {module or '(target root)'}")
            print(f"  filter: {selection.expression()}")
    except (ResolutionError, SourceError) as error:
        print(f"owned-tests: {error}", file=sys.stderr)
        return 3
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))