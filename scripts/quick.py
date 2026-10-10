#!/usr/bin/env python3
"""Quick: run the tests of the modules a change touched, and stop at the first failure.

Quick is the check an agent runs after each batch of edits. It picks the tests
from the changed Rust files, runs them without instrumentation under the `quick`
nextest profile, and stops at the first failure. It judges no coverage. CI owns
the full test and coverage verdict, so a failure in a dependent crate shows up
there and not here.

    python3 scripts/quick.py --base origin/master            # plan, then run
    python3 scripts/quick.py --base origin/master --plan     # plan only
    python3 scripts/quick.py --base origin/master --plan --json
    python3 scripts/quick.py --base origin/master --plan --closure
    python3 scripts/quick.py --base origin/master --retry-failed

The mapping from a file to its tests is `owned_tests.owned_selection`, which
reads the module declarations the compiler obeys, so a `#[path]` rename is
followed. The plan prints the estimate from the last run's per-test times; it
builds nothing and runs nothing. `--closure` estimates the packages `make
coverage-fast` would run, which is how an agent decides whether the optional
Done check fits its 3 minute budget.
"""

from __future__ import annotations

import argparse
import functools
import json
import shlex
import subprocess
import sys
from pathlib import Path
from typing import Callable, NamedTuple, Sequence

sys.path.insert(0, str(Path(__file__).resolve().parent))

import owned_tests as owned  # noqa: E402
import scoped_runner as runner  # noqa: E402
from owned_scanner import SourceError  # noqa: E402
from quick_estimate import Case, Estimate, estimate, failed_cases, newest_history, wall_seconds  # noqa: E402
from quick_terms import Term, failed_expression, union_expression  # noqa: E402

# `owned_selection` re-walks the whole crate, and re-runs `cargo metadata`, for
# every file it is asked about. One of each is enough for one run, and without
# this a large diff pays for the same work once per changed file.
owned.walk_crate = functools.lru_cache(maxsize=None)(owned.walk_crate)
owned.workspace_crates = functools.lru_cache(maxsize=None)(owned.workspace_crates)

QUICK_BUDGET_S = 60.0
DONE_BUDGET_S = 180.0
THREADS = int(runner.TEST_THREADS)
NEXTEST = runner.ROOT / "target" / "nextest"
QUICK_JUNIT = NEXTEST / "quick" / "junit.xml"
HISTORY = [
    runner.ROOT / "target" / "llvm-cov-target" / "nextest" / "ci" / "junit.xml",
    NEXTEST / "ci" / "junit.xml",
    QUICK_JUNIT,
]
NO_CHANGE = "quick: no changed Rust file"

Lookup = Callable[[str], "owned.OwnedSelection"]


class Selection(NamedTuple):
    terms: list[Term]
    notes: list[str]


def term_for(rel: str, package: str, lookup: Lookup = owned.owned_selection) -> tuple[Term, str]:
    """The term for one changed file, and the line that explains it."""
    path = Path(rel)
    if path.name in ("lib.rs", "main.rs") and path.parent.name == "src":
        return Term(package, kinds=("lib", "bin")), f"{rel} -> {package}: its lib and bin unit tests"
    try:
        found = lookup(rel)
    except (owned.ResolutionError, SourceError) as error:
        return Term(package, reason=str(error)), f"{rel} -> {package}: whole package, {error}"
    if found.binary:
        return Term(package, binary=found.binary), f"{rel} -> {package}: test target {found.binary}"
    if found.is_empty:
        reason = "the file owns no test module"
        return Term(package, reason=reason), f"{rel} -> {package}: whole package, {reason}"
    modules = tuple(found.modules)
    return Term(package, modules=modules), f"{rel} -> {package}: {len(modules)} test module(s)"


def merge(terms: Sequence[Term]) -> list[Term]:
    """One term per (package, kinds, binary); a whole-package term absorbs the rest."""
    wide = {term.package for term in terms if term.is_wide}
    merged: dict[tuple, Term] = {}
    for term in terms:
        if term.package in wide and not term.is_wide:
            continue
        key = (term.package, term.kinds, term.binary)
        old = merged.get(key)
        modules = tuple(sorted(set(old.modules if old else ()) | set(term.modules)))
        reason = (old.reason if old else "") or term.reason
        merged[key] = term._replace(modules=modules, reason=reason)
    return list(merged.values())


def select(files: Sequence[str], packages: Sequence[dict], lookup: Lookup = owned.owned_selection) -> Selection:
    """The terms for `files`; fails closed on a file no workspace package owns."""
    terms: list[Term] = []
    notes: list[str] = []
    for rel in files:
        owners = runner.package_owners(rel, packages)
        if not owners:
            raise runner.RunError(f"{rel} belongs to no workspace package, so Quick cannot select its tests")
        term, note = term_for(rel, owners[0], lookup)
        terms.append(term)
        notes.append(note)
    return Selection(merge(terms), notes)


def estimate_line(found: Estimate | None, budget: float, label: str) -> list[str]:
    if found is None:
        return [f"quick: {label}: unknown (no per-test times under target/nextest yet)"]
    wall = wall_seconds(found.seconds, THREADS)
    lines = [
        f"quick: {label}: {found.seconds:.1f} s of test time over {found.tests} test(s) with history, "
        f"about {wall:.1f} s at {THREADS} test threads; build time and tests never run are not counted"
    ]
    lines.append(
        f"quick: over the {budget:.0f} s budget" if wall > budget else f"quick: within the {budget:.0f} s budget"
    )
    return lines


def plan_lines(selection: Selection, history: list[Case]) -> list[str]:
    lines = [f"quick: {note}" for note in selection.notes]
    lines.append(f"quick: filter: {union_expression(selection.terms)}")
    keep = lambda terms: (lambda case: any(t.matches(case.binary_id, case.name) for t in terms))  # noqa: E731
    total = estimate(history, keep(selection.terms))
    lines += estimate_line(total, QUICK_BUDGET_S, "estimate")
    if total is not None and wall_seconds(total.seconds, THREADS) > QUICK_BUDGET_S:
        priced = [(estimate(history, keep([term])), term) for term in selection.terms]
        priced = sorted(((e.seconds, t) for e, t in priced if e), key=lambda pair: -pair[0])
        for seconds, term in priced[:3]:
            lines.append(f"quick:   {seconds:.1f} s  {term.expression()}")
    return lines


def plan_json(selection: Selection, files: Sequence[str]) -> str:
    return json.dumps(
        {
            "files": list(files),
            "filter": union_expression(selection.terms),
            "terms": [
                {"package": t.package, "kinds": list(t.kinds), "binary": t.binary, "patterns": t.patterns()}
                for t in selection.terms
            ],
        },
        indent=2,
    )


def closure_lines(packages: Sequence[str], history: list[Case]) -> list[str]:
    names = set(packages)
    found = estimate(history, lambda case: case.binary_id.split("::")[0] in names)
    lines = [f"quick: closure: {len(packages)} package(s): {', '.join(packages)}"]
    lines += estimate_line(found, DONE_BUDGET_S, "closure estimate")
    lines.append("quick: the Done run is instrumented, so this estimate likely reads low")
    if found is not None and wall_seconds(found.seconds, THREADS) > DONE_BUDGET_S:
        lines.append("quick: leave the coverage verdict to CI")
    return lines


def nextest(packages: Sequence[str], expression: str, extra: Sequence[str] = ()) -> int:
    """Run `expression` over `packages`. Without `-p`, cargo builds and runs only the
    workspace's default member, so the packages must be named."""
    command = ["cargo", "nextest", "run", "--locked", "--profile", "quick", *extra]
    command += [flag for name in packages for flag in ("-p", name)] + ["-E", expression]
    print("running:", shlex.join(command), flush=True)
    return subprocess.run(command, cwd=runner.ROOT, env=runner.hermetic_env()).returncode


def run_selection(selection: Selection, retry_failed: bool) -> int:
    if retry_failed:
        failed = failed_cases(QUICK_JUNIT)
        if failed:
            print(f"quick: retrying the {len(failed)} test(s) that failed last time", flush=True)
            # A failure from a test since removed or renamed matches nothing, and
            # that is not a failure: carry on to the full selection.
            code = nextest(
                sorted({binary.split("::")[0] for binary, _ in failed}),
                failed_expression(failed),
                ("--no-tests=pass",),
            )
            if code != 0:
                return code
        else:
            print("quick: no failed test in the last Quick run; running the full selection", flush=True)
    return nextest(sorted({term.package for term in selection.terms}), union_expression(selection.terms))


def parse(argv: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run the tests of the modules a change touched.")
    parser.add_argument("--base", required=True, help="ref to diff against, e.g. origin/master")
    parser.add_argument("--plan", action="store_true", help="print the selection and the estimate, run nothing")
    parser.add_argument("--json", action="store_true", help="with --plan, print the selection as JSON")
    parser.add_argument("--closure", action="store_true", help="with --plan, estimate the coverage-fast closure")
    parser.add_argument("--retry-failed", action="store_true", help="run last run's failures first")
    args = parser.parse_args(argv)
    if (args.json or args.closure) and not args.plan:
        parser.error("--json and --closure need --plan")
    if args.json and args.closure:
        parser.error("--json and --closure cannot be combined")
    return args


def run(args: argparse.Namespace) -> int:
    if runner.resolve_ref(args.base) is None:
        raise runner.RunError(f"base ref {args.base!r} resolves to no commit, so the changed files cannot be determined")
    history = newest_history(HISTORY)
    if args.closure:
        packages = runner.select(args.base, False)[0]
        if not packages:
            print(NO_CHANGE)
            return runner.EXIT_OK
        print("\n".join(closure_lines(packages, history)))
        return runner.EXIT_OK
    files = [rel for rel in runner.changed_files(args.base) if runner.is_present(rel)]
    if not files:
        print(NO_CHANGE)
        return runner.EXIT_OK
    selection = select(files, runner.workspace_packages())
    if args.json:
        print(plan_json(selection, files))
        return runner.EXIT_OK
    print("\n".join(plan_lines(selection, history)), flush=True)
    return runner.EXIT_OK if args.plan else run_selection(selection, args.retry_failed)


def main(argv: Sequence[str]) -> int:
    args = parse(argv)
    try:
        return run(args)
    except runner.RunError as error:
        print(f"quick: {error}", file=sys.stderr)
        return runner.EXIT_RUN_ERROR


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
