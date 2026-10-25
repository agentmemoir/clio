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
    python3 scripts/quick.py --base origin/master --plan --closure --json
    python3 scripts/quick.py --base origin/master --retry-failed

The mapping from a file to its tests is `owned_tests.owned_selection`, which
reads the module declarations the compiler obeys, so a `#[path]` rename is
followed. A file selects only the test modules it declares; a file that owns
no test module selects no test, and the plan says so. The plan prints the estimate from the last run's per-test times, read
from this checkout and from the main checkout when this is a linked worktree;
it builds nothing and runs nothing. `--closure` estimates the tests `make
coverage-fast` would run, with the build and coverage cost counted, which is how
an agent decides whether the optional Done check fits its 3 minute budget. The
gate asks for the same estimate with `--json` and refuses an agent's run that
does not fit.
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
from precommit_hooks import main_checkout  # noqa: E402
from gate_done_check import fresh_records  # noqa: E402
from quick_estimate import Case, Estimate, estimate, failed_cases, newest_history, wall_seconds  # noqa: E402
from quick_terms import Term, failed_expression, union_expression  # noqa: E402
import quick_untested as untested  # noqa: E402

# `owned_selection` re-walks the whole crate, and re-runs `cargo metadata`, for
# every file it is asked about. One of each is enough for one run, and without
# this a large diff pays for the same work once per changed file.
owned.walk_crate = functools.lru_cache(maxsize=None)(owned.walk_crate)
owned.workspace_crates = functools.lru_cache(maxsize=None)(owned.workspace_crates)

QUICK_BUDGET_S = 60.0
DONE_BUDGET_S = 180.0
# A Done run also builds the instrumented packages and writes the report, and the gate deletes those
# objects before each run, so that cost repeats. Without a fresh run of this worktree that recorded its
# test time, the build cost is this constant: the largest gate-clock build and report measured on
# 2026-10-21 at load 34 on 10 cores.
BUILD_FALLBACK_S = 53.0
THREADS = int(runner.TEST_THREADS)
NEXTEST = runner.ROOT / "target" / "nextest"
QUICK_JUNIT = NEXTEST / "quick" / "junit.xml"
HISTORY = (
    Path("target/nextest/ci/junit.xml"),
    Path("target/nextest/quick/junit.xml"),
)
NO_CHANGE = "quick: no changed Rust file"
NO_TEST = "quick: no test selected, so nothing runs"
SCHEMA_CHECK = runner.ROOT / "scripts" / "schema_args_check.py"
# Test targets a change reaches across a package boundary. `crates/clio/tests/golden_cli_help.json`
# embeds every MCP tool description, so a `clio-mcp` source change can stale a byte comparison in
# `clio` that the changed file reaches and does not own, and no module declaration can say so. A
# path prefix, the package that holds the target, and the target.
CROSS_PACKAGE_TESTS = (
    ("crates/clio-mcp/src/", "clio", "golden_repeatability_test"),
)

Lookup = Callable[[str], "owned.OwnedSelection"]


class Selection(NamedTuple):
    terms: list[Term]
    notes: list[str]


def term_for(rel: str, package: str, lookup: Lookup = owned.owned_selection) -> tuple[Term | None, str]:
    """The term for the test modules one changed file declares, or None, and the line that explains it."""
    try:
        found = lookup(rel)
    except (owned.ResolutionError, SourceError) as error:
        return None, f"no test selected: {rel} owns no test module ({error})"
    if found.binary:
        return Term(package, binary=found.binary), f"{rel} -> {package}: test target {found.binary}"
    if found.is_empty:
        if "tests" in Path(rel).parts:
            return None, f"no test selected: {rel} is a test file with no test of its own"
        return None, f"no test selected: {rel} owns no test module"
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
        terms += [term] if term else []
        notes.append(note)
    for prefix, package, binary in CROSS_PACKAGE_TESTS:
        reaching = [rel for rel in files if rel.startswith(prefix)]
        if reaching:
            terms.append(Term(package, binary=binary))
            notes.append(f"{', '.join(reaching)} -> {package}: test target {binary}, reached and not owned")
    return Selection(merge(terms), notes)


def history_files() -> list[Path]:
    """The JUnit files nextest wrote here and, in a linked worktree, in the main checkout.

    A new worktree has run nothing, so alone it would never have a time to estimate from.
    """
    roots = [runner.ROOT]
    main = main_checkout(runner.ROOT)
    if main is not None:
        roots.append(main)
    return [root / relative for root in roots for relative in HISTORY]


def estimate_line(found: Estimate | None, budget: float, label: str) -> list[str]:
    if found is None:
        return [f"quick: {label}: unknown (no per-test times under target/nextest yet)"]
    threaded = wall_seconds(found.seconds, THREADS)
    counted = f"about {threaded:.1f} s at {THREADS} test threads; build time and tests never run are not counted"
    lines = [f"quick: {label}: {found.seconds:.1f} s of test time over {found.tests} test(s) with history, {counted}"]
    lines.append(
        f"quick: over the {budget:.0f} s budget" if threaded > budget else f"quick: within the {budget:.0f} s budget"
    )
    return lines


def plan_lines(selection: Selection, history: list[Case], packages: Sequence[dict]) -> list[str]:
    lines = [f"quick: {note}" for note in selection.notes]
    lines.append(f"quick: filter: {union_expression(selection.terms)}")
    targets = target_flags(selection.terms, packages)
    if targets:
        lines.append(f"quick: cargo targets: {' '.join(targets)}")
    total = estimate(history, keeps(selection.terms))
    lines += estimate_line(total, QUICK_BUDGET_S, "estimate")
    if total is not None and wall_seconds(total.seconds, THREADS) > QUICK_BUDGET_S:
        priced = [(estimate(history, keeps([term])), term) for term in selection.terms]
        priced = sorted(((e.seconds, t) for e, t in priced if e), key=lambda pair: -pair[0])
        for seconds, term in priced[:3]:
            lines.append(f"quick:   {seconds:.1f} s  {term.expression()}")
    return lines


def plan_json(selection: Selection, files: Sequence[str], left_out: Sequence[str] = ()) -> str:
    return json.dumps(
        {
            "files": list(files),
            "left_out": list(left_out),
            "filter": union_expression(selection.terms),
            "terms": [
                {"package": t.package, "kinds": list(t.kinds), "binary": t.binary, "patterns": t.patterns()}
                for t in selection.terms
            ],
        },
        indent=2,
    )


def keeps(terms: Sequence[Term]) -> Callable[[Case], bool]:
    """The history predicate for the tests `terms` select: the rule the run's filter applies."""
    return lambda case: any(term.matches(case.binary_id, case.name) for term in terms)


def closure_estimate(terms: Sequence[Term], history: list[Case]) -> Estimate | None:
    return estimate(history, keeps(terms))


class Done(NamedTuple):
    """What a Done run likely takes: its build and report, then its tests at THREADS workers."""

    build: float
    test: float
    tests: int
    source: str

    @property
    def seconds(self) -> float:
        return self.build + self.test


def build_estimate() -> tuple[float, str]:
    """The build and report seconds of a Done run here, and where that number came from.

    The newest fresh run of this worktree that recorded its test time gives it: the run's wall time
    less its test time. Without one, BUILD_FALLBACK_S.
    """
    for run in reversed(fresh_records(str(runner.ROOT))):
        started, finished, test_s = run.get("started"), run.get("finished"), run.get("test_s")
        if all(isinstance(value, (int, float)) for value in (started, finished, test_s)) and finished - started > test_s:
            return finished - started - test_s, "learned from this worktree's last fresh run"
    return BUILD_FALLBACK_S, "fallback constant, no fresh run here recorded a test time"


def done_run(terms: Sequence[Term], history: list[Case]) -> Done | None:
    """The Done estimate for these terms, or None when no history says how long their tests take."""
    if not terms:
        return Done(0.0, 0.0, 0, "nothing to run")
    found = closure_estimate(terms, history)
    if found is None:
        return None
    build, source = build_estimate()
    return Done(build, wall_seconds(found.seconds, THREADS), found.tests, source)


def closure_lines(terms: Sequence[Term], history: list[Case]) -> list[str]:
    done = done_run(terms, history)
    packages = sorted({term.package for term in terms})
    lines = [f"quick: closure: {len(packages)} package(s): {', '.join(packages)}"]
    if done is None:
        lines.append("quick: closure estimate: unknown (no per-test times under target/nextest yet)")
        return lines
    lines.append(
        f"quick: closure estimate: {done.seconds:.1f} s, which is build and report {done.build:.1f} s "
        f"({done.source}) plus tests {done.test:.1f} s over {done.tests} test(s)"
    )
    if done.seconds > DONE_BUDGET_S:
        lines += [f"quick: over the {DONE_BUDGET_S:.0f} s budget", "quick: leave the coverage verdict to CI"]
    else:
        lines.append(f"quick: within the {DONE_BUDGET_S:.0f} s budget")
    return lines


def closure_json(terms: Sequence[Term], history: list[Case]) -> str:
    """The closure estimate for a program: `within` is null when there is no history to judge by.

    With no test selected there is nothing to run, so the run takes no time.
    """
    done = done_run(terms, history)
    return json.dumps(
        {
            "packages": sorted({term.package for term in terms}),
            "tests": done.tests if done is not None else 0,
            "build_seconds": None if done is None else done.build,
            "test_seconds": None if done is None else done.test,
            "build_source": None if done is None else done.source,
            "done_seconds": None if done is None else done.seconds,
            "budget_seconds": DONE_BUDGET_S,
            "within": None if done is None else done.seconds <= DONE_BUDGET_S,
        },
        indent=2,
    )


def schema_check(files: Sequence[str]) -> int:
    """Run the static schema check first when the change touches `clio-mcp`; its exit code."""
    if not any(rel.startswith("crates/clio-mcp/") for rel in files):
        return 0
    return subprocess.run([sys.executable, str(SCHEMA_CHECK)], cwd=runner.ROOT).returncode


def target_flags(terms: Sequence[Term], packages: Sequence[dict]) -> list[str]:
    """Cargo target filters for a selection.

    Unit tests live in the lib and bin targets, and an integration test in its own
    target. Only those are built. Without this, a one-module change relinked every
    integration binary of the package and ran each once to list its tests, and that
    took minutes on a package with twenty of them.

    Cargo applies the filters to every selected package, so a whole-package term
    names each integration target of its own package. Dropping the filters for it
    would build every target of the other packages too.
    """
    if all(term.is_wide for term in terms):
        return []
    wide = {term.package for term in terms if term.is_wide}
    names = {term.binary for term in terms if term.binary}
    names.update(name for package in packages if package["name"] in wide for name in package["tests"])
    flags = ["--lib", "--bins"] if wide or any(term.modules or term.kinds for term in terms) else []
    for name in sorted(names):
        flags += ["--test", name]
    return flags


def failed_target_flags(failed: Sequence[tuple[str, str]]) -> list[str]:
    """Cargo target filters for the binaries that held the failed tests."""
    targets: set[tuple[str, ...]] = set()
    for binary_id in {binary for binary, _ in failed}:
        _, _, target = binary_id.partition("::")
        if not target:
            targets.add(("--lib",))
        elif target.startswith("bin/"):
            targets.add(("--bin", target[4:]))
        else:
            targets.add(("--test", target))
    return [flag for entry in sorted(targets) for flag in entry]


def nextest(
    packages: Sequence[str], expression: str, extra: Sequence[str] = (), targets: Sequence[str] = ()
) -> int:
    """Run `expression` over `packages`. Naming the packages keeps the run to them; the
    root manifest has no `default-members`, so a bare run would select every member."""
    command = ["cargo", "nextest", "run", "--locked", "--profile", "quick", *extra]
    command += [flag for name in packages for flag in ("-p", name)] + [*targets, "-E", expression]
    print("running:", shlex.join(command), flush=True)
    return subprocess.run(command, cwd=runner.ROOT, env=runner.hermetic_env()).returncode


def run_selection(selection: Selection, packages: Sequence[dict], retry_failed: bool) -> int:
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
                failed_target_flags(failed),
            )
            if code != 0:
                return code
        else:
            print("quick: no failed test in the last Quick run; running the full selection", flush=True)
    return nextest(
        sorted({term.package for term in selection.terms}),
        union_expression(selection.terms),
        targets=target_flags(selection.terms, packages),
    )


def parse(argv: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run the tests of the modules a change touched.")
    parser.add_argument("--base", required=True, help="ref to diff against, e.g. origin/master")
    parser.add_argument("--plan", action="store_true", help="print the selection and the estimate, run nothing")
    parser.add_argument("--json", action="store_true", help="with --plan, print the selection (with --closure, the estimate) as JSON")
    parser.add_argument("--closure", action="store_true", help="with --plan, estimate the coverage-fast closure")
    parser.add_argument("--retry-failed", action="store_true", help="run last run's failures first")
    args = parser.parse_args(argv)
    if (args.json or args.closure) and not args.plan:
        parser.error("--json and --closure need --plan")
    return args


def run(args: argparse.Namespace) -> int:
    if runner.resolve_ref(args.base) is None:
        raise runner.RunError(f"base ref {args.base!r} resolves to no commit, so the changed files cannot be determined")
    history = newest_history(history_files())
    if args.closure:
        _, terms, _, _ = runner.coverage_selection(args.base)
        if args.json:
            print(closure_json(terms, history))
            return runner.EXIT_OK
        if not terms:
            print(NO_TEST)
            return runner.EXIT_OK
        print("\n".join(closure_lines(terms, history)))
        return runner.EXIT_OK
    changed = runner.changed_files(args.base)
    files = [rel for rel in changed if runner.is_present(rel)]
    packages = runner.workspace_packages()
    extra = untested.classify(args.base, packages, [rel for rel in changed if not runner.is_present(rel)])
    left_out = untested.left_out_lines(extra.left_out)
    if not files and not extra.terms:
        print("\n".join(left_out or [NO_CHANGE]))
        return runner.EXIT_OK
    found = select(files, packages)
    selection = Selection(merge([*found.terms, *extra.terms]), [*found.notes, *extra.notes])
    if args.json:
        print(plan_json(selection, files, extra.left_out))
        return runner.EXIT_OK
    if not selection.terms:
        print("\n".join([*(f"quick: {note}" for note in selection.notes), *left_out, NO_TEST]))
        return runner.EXIT_OK
    print("\n".join([*plan_lines(selection, history, packages), *left_out]), flush=True)
    if args.plan:
        return runner.EXIT_OK
    return schema_check(files) or run_selection(selection, packages, args.retry_failed)


def main(argv: Sequence[str]) -> int:
    args = parse(argv)
    try:
        return run(args)
    except runner.RunError as error:
        print(f"quick: {error}", file=sys.stderr)
        return runner.EXIT_RUN_ERROR


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
