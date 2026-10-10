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
import re
import subprocess
import sys
from pathlib import Path
from typing import Iterable, NamedTuple

sys.path.insert(0, str(Path(__file__).resolve().parent))

from integration_targets import (  # noqa: E402
    declared_targets,
    modules_in_target,
    target_of,
)
from owned_scanner import (  # noqa: E402
    Declaration,
    PLATFORM_SKIPPED,
    SourceError,
    children_dir,
    declarations,
    holds_tests,
    holds_tests_text,
    is_support_file,
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


def runner_test_modules() -> tuple[dict[str, set[str]], dict[str, set[str]]]:
    """Real test modules, split into unit-test targets and integration targets.

    Read from the runner rather than predicted from source, because the runner is
    the only thing that knows what actually compiles. A prediction checked only
    against itself passes precisely when the prediction is wrong.

    The split matters because the two kinds need different keys. A unit-test module
    is identified by its path inside the crate; an integration test target is
    identified by its Cargo target name, and 21 targets in one package share the
    empty module path, so path alone cannot tell them apart.

    Stderr is discarded because cargo writes build progress there, and one of
    those lines is not a test row. Counting it as one produced a phantom lost
    test here, and cost a round of chasing a bug that did not exist.
    """
    proc = subprocess.run(
        ["cargo", "nextest", "list", "--workspace", "--locked"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        raise ResolutionError(f"'cargo nextest list' failed: {proc.stderr.strip()}")
    unit: dict[str, set[str]] = {}
    integration: dict[str, set[str]] = {}
    for line in proc.stdout.splitlines():
        parts = line.split()
        if len(parts) != 2:
            continue
        binary, test_name = parts
        package, _, target = binary.partition("::")
        # `pkg` is the lib target and `pkg::bin/<name>` the bin target; anything
        # else with a target segment is an integration test target.
        is_integration = bool(target) and not target.startswith("bin/")
        segments = test_name.split("::")
        module = "::".join(segments[:-1]) if len(segments) > 1 else ""
        if is_integration:
            integration.setdefault(binary, set()).add(module)
        else:
            unit.setdefault(package, set()).add(module)
    return unit, integration


def accept_against_runner() -> int:
    """Check the prediction against the runner's real test modules.

    Three things are reported, because each is a different failure: a real module
    no file claims is a test that would be silently skipped; a predicted module
    holding no test is harmless in a wide filter and fatal alone; a module
    compiled only elsewhere cannot be selected here at all.
    """
    unit_real, integration_real = runner_test_modules()
    predicted: dict[str, set[str]] = {}
    for package, crate_dir in workspace_crates().items():
        try:
            owners = crate_test_modules(crate_dir)
        except (ResolutionError, SourceError) as error:
            print(f"  {package}: could not resolve: {error}")
            continue
        claimed: set[str] = set()
        for modules in owners.values():
            claimed.update(modules)
        if claimed:
            predicted[package] = claimed

    platform_excluded = set(PLATFORM_SKIPPED)
    missing_total = 0
    extra_total = 0
    for package in sorted(set(unit_real) | set(predicted)):
        have = unit_real.get(package, set())
        claim = predicted.get(package, set())
        missing = sorted(m for m in have - claim if m)
        extra = sorted(claim - have)
        if missing or extra:
            print(f"  {package}: {len(missing)} unclaimed, {len(extra)} claimed-but-absent")
        for module in missing:
            print(f"      UNCLAIMED  {module}")
            missing_total += 1
        for module in extra:
            if module in platform_excluded:
                # Already reported as compiled only elsewhere, and deliberately not
                # selected here. Counting it twice would report one defect twice.
                continue
            print(f"      ABSENT     {module}")
            extra_total += 1

    # Integration targets are checked separately and keyed by target name. They
    # were absent from this check entirely at first, which is how a gap survived a
    # check that read "0 unclaimed".
    declared = declared_targets(workspace_crates())
    known_targets = {binary.split("::", 1)[1] for binary in integration_real}
    integration_missing = 0
    for binary in sorted(integration_real):
        target = binary.split("::", 1)[1]
        have = integration_real[binary]
        claim = declared.get(target)
        if claim is None:
            print(f"      UNCLAIMED TARGET  {binary}")
            integration_missing += 1
            continue
        for module in sorted(m for m in have - claim if m):
            print(f"      UNCLAIMED  {binary} :: {module or '(root)'}")
            integration_missing += 1
        for module in sorted(claim - have):
            print(f"      ABSENT     {binary} :: {module or '(root)'}")
            extra_total += 1
    empty_targets = sorted(set(declared) - known_targets)
    if empty_targets:
        print(
            f"  {len(empty_targets)} integration target(s) under tests/ report no tests: "
            f"{', '.join(empty_targets)}"
        )
    skipped = sorted(platform_excluded)
    if skipped:
        print(
            f"  {len(skipped)} module(s) are compiled only on another platform, so no "
            f"test in them runs here and they are not selected:"
        )
        for module in skipped:
            print(f"      OTHER PLATFORM  {module}")
    total_missing = missing_total + integration_missing
    print(
        f"acceptance: {total_missing} test module(s) no file claims "
        f"({missing_total} unit, {integration_missing} in integration targets), "
        f"{extra_total} claimed module(s) hold no test, "
        f"{len(skipped)} compiled only elsewhere, "
        f"{len(integration_real)} integration target(s) checked"
    )
    return 1 if total_missing else 0


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