#!/usr/bin/env python3
"""Acceptance of the test lookup against the runner's real test modules.

Split out of owned_tests.py so that file stays within its size limit. The only
entry point is accept_against_runner, reached as `owned_tests.py --accept`.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from integration_targets import declared_targets  # noqa: E402
from gate_kinds import ANSI_ESCAPE  # noqa: E402
from owned_scanner import (  # noqa: E402
    PLATFORM_SKIPPED,
    SourceError,
    holds_tests,
    is_support_file,
)
from owned_tests import (  # noqa: E402
    ROOT,
    ResolutionError,
    crate_test_modules,
    walk_crate,
    workspace_crates,
)

#: The gate wrapper must never serve a cached verdict here. A replayed lookup
#: publishes log chatter instead of the test list, and the parser below would
#: read that chatter as an empty runner and report a vacuous all-clean.
_LOOKUP_ENV = "GATE_RAW"


def _lookup_env() -> dict[str, str]:
    return {**os.environ, _LOOKUP_ENV: "1"}


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
        env=_lookup_env(),
    )
    if proc.returncode != 0:
        raise ResolutionError(f"'cargo nextest list' failed: {proc.stderr.strip()}")
    # A tool colours its output at the caller's setting (CI sets
    # CARGO_TERM_COLOR=always), so colour bytes are stripped before parsing:
    # a coloured binary id would otherwise key every module under a name the
    # predictions never use, and the whole workspace would report unclaimed.
    unit: dict[str, set[str]] = {}
    integration: dict[str, set[str]] = {}
    for line in ANSI_ESCAPE.sub("", proc.stdout).splitlines():
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

    Five things are reported: a real module no file claims; a predicted module
    holding no test; a module compiled only elsewhere; a shared-helpers file
    holding tests, which breaks the exemption the owner rule depends on; and a
    file holding tests inside a shared tests/ directory that builds no target.
    """
    unit_real, integration_real = runner_test_modules()
    predicted: dict[str, set[str]] = {}
    crates = workspace_crates()
    for package, crate_dir in crates.items():
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
    # Reviewed 2026-10-10 per fix-inline-exceptions: PLATFORM_SKIPPED stays;
    # gate.py ENV_KEY_EXEMPT stays too (read-but-unhashed env, guarded by self-test).
    support_violations: list[str] = []
    for package, crate_dir in crates.items():
        try:
            for path in walk_crate(crate_dir):
                if is_support_file(path.name) and holds_tests(path):
                    support_violations.append(str(path.relative_to(ROOT)))
        except (ResolutionError, SourceError) as error:
            print(f"  {package}: could not resolve: {error}")
            continue
    shared_dir_violations: list[str] = []
    for package, crate_dir in crates.items():
        tests_dir = crate_dir / "tests"
        if not tests_dir.is_dir():
            continue
        for entry in sorted(tests_dir.iterdir()):
            if not entry.is_dir() or (entry / "main.rs").exists():
                continue
            for candidate in sorted(entry.glob("*.rs")):
                if candidate.name == "mod.rs":
                    continue
                try:
                    if holds_tests(candidate):
                        shared_dir_violations.append(str(candidate.relative_to(ROOT)))
                except SourceError as error:
                    print(f"  {package}: could not read: {error}")
                    continue
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
    declared = declared_targets(crates)
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
    if support_violations:
        print(f"  {len(support_violations)} shared-helpers file(s) hold tests:")
        for rel in sorted(support_violations):
            print(
                f"      SUPPORT    {rel}: is a shared-helpers file but holds tests; "
                "give it an owner and rename it to *_tests.rs, or move the tests out"
            )
    if shared_dir_violations:
        print(f"  {len(shared_dir_violations)} shared tests/ file(s) hold tests:")
        for rel in sorted(shared_dir_violations):
            print(
                f"      SHARED-DIR {rel}: sits in a shared tests/ directory that builds "
                "no test target, so it has no owner, and it holds tests"
            )
    print(
        f"acceptance: {total_missing} test module(s) no file claims "
        f"({missing_total} unit, {integration_missing} in integration targets), "
        f"{extra_total} claimed module(s) hold no test, "
        f"{len(skipped)} compiled only elsewhere, "
        f"{len(support_violations)} shared-helpers file(s) hold tests, "
        f"{len(shared_dir_violations)} shared tests/ file(s) hold tests, "
        f"{len(integration_real)} integration target(s) checked"
    )
    return 1 if total_missing or support_violations or shared_dir_violations else 0
