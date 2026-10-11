"""Self-test cases for git references, renames and the plan modes of main."""

from __future__ import annotations

from pathlib import Path, PurePosixPath

from scoped_core import CoverageRow, RunError, owned
from scoped_judge import judge, report_path_for
from scoped_run import TEST_THREADS, coverage_report_command, coverage_test_command
from scoped_select import resolve_ref, select

from scoped_selftest_support import shared as globals


def run(check) -> tuple:
    # The fail-closed paths. Each one is a way the tool can end up measuring
    # nothing, and each must be an error rather than a clean run. These are the
    # cases that matter most, because an empty scope is indistinguishable from
    # a pass in the output.
    class Boom(RunError):
        pass

    def raises(fn, *a, **kw) -> bool:
        try:
            fn(*a, **kw)
        except RunError:
            return True
        return False

    original_ref_exists = globals()["_ref_exists"]
    original_merge_base = globals()["merge_base"]
    original_changed = globals()["changed_files"]
    try:
        globals()["_ref_exists"] = lambda ref: False
        globals()["merge_base"] = lambda base: None
        check("a ref that does not exist is an error", raises(select, "no-such-ref", False), True)

        # The case that broke CI. A checkout in detached HEAD mode has no local
        # branch for the base, only refs/remotes/origin/<base>, so the bare name
        # does not resolve while the remote form does. Refusing here would fail
        # every pull request; treating it as an empty scope would pass over
        # nothing. The remote form has to be found and used.
        globals()["_ref_exists"] = lambda ref: ref.startswith("origin/")
        check(
            "a bare ref that only exists as a remote-tracking ref still resolves",
            resolve_ref("master"),
            "origin/master",
        )
        check(
            "the remote fallback makes select() succeed where the bare name fails",
            raises(select, "master", False),
            False,
        )

        globals()["_ref_exists"] = lambda ref: True
        # A changed .rs file that no package owns can never be measured, so it
        # must be an error rather than a silent all-clear.
        globals()["changed_files"] = lambda base: ["stray/tool.rs"]
        check(
            "a changed Rust file no package owns is an error",
            raises(select, "master", False),
            True,
        )

        # A change with no Rust file at all is a legitimate all-clear: docs, a
        # Makefile, or a Python script cannot move a Rust line count.
        globals()["changed_files"] = lambda base: []
        globals()["workspace_packages"] = lambda: [{"name": "a", "deps": set()}]
        packages, files, changed = select("master", False)
        check("a change with no Rust file selects nothing", (packages, files, changed), ([], [], []))

        # A change that touches ONLY test-named files. llvm-cov path-excludes
        # those, so nothing is judgeable, but the tests still have to run: seeding
        # the closure from judgeable files alone selected no package here and the
        # gate would have passed a commit that added a test without ever running
        # it. Measured over 26 past bug-fix commits, this was one of two commits
        # the scoping would not have covered, and the other was a fixed one.
        globals()["workspace_packages"] = lambda: [
            {"name": "a", "dir": "crates/a", "deps": set()},
            {"name": "b", "dir": "crates/b", "deps": {"a"}},
        ]
        globals()["changed_files"] = lambda base: ["crates/a/src/thing_tests.rs"]
        packages, files, changed = select("master", False)
        check(
            "a test-only change still selects its package and its dependents",
            (packages, files),
            (["a", "b"], []),
        )
        # The same must hold for an integration test, which the suffix half alone
        # let through into the judged list. It is path-excluded, so it can never
        # be measured, but it still has to select its package so the test runs.
        globals()["changed_files"] = lambda base: ["crates/a/tests/harness.rs"]
        packages, files, changed = select("master", False)
        check(
            "an integration-test change selects its package and is not judged",
            (packages, files, changed),
            (["a", "b"], [], ["crates/a/tests/harness.rs"]),
        )

        # A file the change deleted still selects its package, so the crate's tests
        # run, but it is not judged: nothing is left in it for a report to hold. The
        # presence test reads the real tree, so the surviving half of the pair is a
        # path that exists and the other is one that cannot.
        globals()["workspace_packages"] = lambda: [
            {"name": "clio-types", "dir": "crates/clio-types", "deps": set()},
        ]
        globals()["changed_files"] = lambda base: [
            "crates/clio-types/src/civil.rs",
            "crates/clio-types/src/retired.rs",
        ]
        packages, files, changed = select("master", False)
        check(
            "a deleted file still selects its package but is not judged",
            (packages, files),
            (["clio-types"], ["crates/clio-types/src/civil.rs"]),
        )
        check(
            "a deleted file is still reported as changed",
            changed,
            ["crates/clio-types/src/civil.rs", "crates/clio-types/src/retired.rs"],
        )
    finally:
        globals()["_ref_exists"] = original_ref_exists
        globals()["merge_base"] = original_merge_base
        globals()["changed_files"] = original_changed

    # There is no per-file floor to ratchet and no aggregate to compare against.
    # Both used to be rules here, and both are gone, so the cases that pinned them
    # are replaced by the ones that hold the line in place: the same row judged at
    # the same number from the same function, whatever the caller passes.
    check(
        "the judge reads LINE_FLOOR when no floor is passed",
        judge({"crates/a/src/known.rs": CoverageRow(79.99, 100.0)}, ["crates/a/src/known.rs"])[0],
        ["crates/a/src/known.rs lines 79.99% (floor 80.00)"],
    )


    # The test-concurrency cap is a flake mitigation. If it is ever dropped by
    # accident, the shared-Postgres suites get their contention back, so its
    # presence is asserted rather than assumed.
    check("both modes cap test concurrency", TEST_THREADS, "2")

    # An integration target is selected by its Cargo target name, and its root-level
    # tests share the empty module path with every other integration target in the
    # same package. A name-only filter would therefore select all 20-odd of them, so
    # the binary predicate is load-bearing and is asserted here.
    check(
        "an integration target with no module path still gets a binary predicate",
        owned.filter_expression([], "retention_profile_harness"),
        "binary(retention_profile_harness)",
    )
    check(
        "an integration target with modules combines binary and name predicates",
        owned.filter_expression(["tests"], "conformance"),
        "binary(conformance) and test(/^tests(::|$)/)",
    )
    check(
        "an integration filter is anchored inside its target",
        owned.filter_expression(["tests"], "conformance").count("binary("),
        1,
    )

    # Per-file attribution. Two changed files must not share one report, or the
    # second file's number stands in for the first's. Measured before this was
    # fixed: create.rs read 81.82% alone and 92.12% when a second file's tests
    # joined the same run, which is its whole-crate figure.
    first = report_path_for("crates/a/src/first.rs")
    second = report_path_for("crates/a/src/second.rs")
    check("two files get two report paths", first != second, True)
    check("a report path is per file", first.name.startswith("coverage-file-"), True)
    check(
        "the same file always resolves to the same report path",
        report_path_for("crates/a/src/first.rs"),
        first,
    )
    check(
        "a path with no modules and no target yields no filter",
        owned.filter_expression([], None),
        "",
    )

    # --- the one rule: a LINE floor, and nothing else --------------------------
    #
    # The operator withdrew the function floor, and these cases pin what that
    # means at the judge in both directions: a file that would have failed on
    # functions alone now passes, and a file that fails on lines alone still
    # fails. A change that reintroduced a function comparison would leave the
    # second case passing, and a change that only removed the function fraction
    # from the message would leave the first passing for the wrong reason.
    #
    # Both scopes reach the same `judge`, so there is one of these rather than
    # two: one scope judging on functions would be a gate with two answers to the
    # same question.
    function_only = {"crates/a/src/half.rs": CoverageRow(91.0, 40.0, 100, 5, 91, 2)}
    check(
        "a file whose only shortfall is function coverage passes",
        judge(function_only, ["crates/a/src/half.rs"]),
        ([], []),
    )
    lines_only_short = {"crates/a/src/half.rs": CoverageRow(79.9, 100.0, 100, 5, 79, 5)}
    lines_offenders, _ = judge(lines_only_short, ["crates/a/src/half.rs"])
    check("a file short on lines alone still fails", len(lines_offenders), 1)
    check("the offender names the line floor it missed", "(floor 80.00)" in lines_offenders[0], True)


    # --- the two coverage steps, and what each may and may not carry ---------
    #
    # The split is the whole reason this gate can act on a run, so each flag is
    # asserted rather than assumed. A flag carried by the wrong step is a silent
    # failure in one direction or the other: a report written by the test step
    # cannot be cleaned up before the merge, and a merge inside the test step
    # cannot be cleaned up at all.
    test_cmd = coverage_test_command(["clio-types", "clio-store"], "test(/^a::tests(::|$)/)")
    report_cmd = coverage_report_command(Path("/tmp/report.json"))
    check("the test step is a nextest run", test_cmd[:3], ["cargo", "llvm-cov", "nextest"])
    check(
        "the test step names both packages",
        [test_cmd[i + 1] for i, flag in enumerate(test_cmd) if flag == "-p"],
        ["clio-types", "clio-store"],
    )
    check("the test step writes no report", "--no-report" in test_cmd, True)
    check("the test step exports no json", "--json" in test_cmd, False)
    check("the test step names no report path", "--output-path" in test_cmd, False)
    check(
        "the test step passes no failure mode",
        [flag for flag in test_cmd if flag.startswith("--failure-mode")],
        [],
    )
    # `--no-clean` is rejected together with `--no-report` by the tool itself, so
    # carrying both is a hard error before anything builds rather than a warning.
    check(
        "the test step omits the flag no-report conflicts with",
        "--no-clean" in test_cmd,
        False,
    )
    check("the test step sends exactly one filter", test_cmd.count("-E"), 1)
    check(
        "the filter follows its expression",
        test_cmd[test_cmd.index("-E") + 1],
        "test(/^a::tests(::|$)/)",
    )
    check("the report step is the report subcommand", report_cmd[:3], ["cargo", "llvm-cov", "report"])
    check("the report step exports json", "--json" in report_cmd, True)
    check(
        "the report step names the report path",
        report_cmd[report_cmd.index("--output-path") + 1],
        "/tmp/report.json",
    )
    check(
        "the report step fails only when every buffer is unreadable",
        "--failure-mode=all" in report_cmd,
        True,
    )
    # A package filter does not narrow the tool's object walk, so passing one to
    # the report step would look like it restricted something and change nothing.
    check("the report step selects no package", "-p" in report_cmd, False)
    check("the two steps are different commands", test_cmd == report_cmd, False)
    return raises,
