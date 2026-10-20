"""Self-test cases for package ownership, the codeless-file rules and the verdict report."""

from __future__ import annotations

import io
import sys

from scoped_cli import main
from scoped_core import CoverageRow, EXIT_FAIL, EXIT_OK, LINE_FLOOR, ROOT, offenders, owned
from scoped_judge import judge, report_rows, report_verdicts
from scoped_run import run_tests
from scoped_select import (
    changed_files,
    changed_paths,
    has_no_executable_code,
    is_judgeable,
    is_measureable,
    is_present,
    package_owners,
)

from quick_terms import Term
from scoped_judge import declarations_only, exemption
from scoped_selftest_support import shared as globals


def run(check) -> tuple:
    # A directory-to-package map in which the names deliberately differ, pinning
    # the property that the directory name is never used on its own: handing it
    # to `cargo llvm-cov -p` gives a value that does not resolve. Every directory
    # in this workspace matches its package name, so the mismatch here is
    # synthetic. The dirs are repository-relative because that is the form
    # `workspace_packages` stores and the form git reports.
    fake = [
        {"name": "example-crate", "deps": set(), "dir": "crates/mismatched-dir"},
        {"name": "clio-store", "deps": set(), "dir": "crates/clio-store"},
    ]
    check(
        "a leaf file belongs to its crate",
        package_owners("crates/clio-store/src/sqlite.rs", fake),
        ["clio-store"],
    )
    check(
        "a crate whose directory name differs maps to its package name",
        package_owners("crates/mismatched-dir/src/lib.rs", fake),
        ["example-crate"],
    )
    check(
        "a nested test file belongs to its crate",
        package_owners("crates/mismatched-dir/tests/harness/mod.rs", fake),
        ["example-crate"],
    )
    check(
        "a file outside crates belongs to nothing",
        package_owners("scripts/scoped_runner.py", fake),
        [],
    )
    check("a *_tests.rs name alone does not remove a file from measurement", is_measureable("crates/clio/src/a_tests.rs"), True)
    check("production files are measured", is_measureable("crates/clio/src/a.rs"), True)
    # A file holding no executable code is a third exclusion, and the two halves
    # it could be confused with are pinned here too. `is_measureable` alone cannot
    # see it, so the source check is separate: without it, every commit adding a
    # `mod` to a crate root failed a gate that reported zero files below its floor.
    check(
        "a file holding no executable code is not judged",
        is_judgeable("crates/clio-types/src/lib.rs", {"crates/clio-types/src/lib.rs"}),
        False,
    )
    check(
        "the same file is judged when the codeless set is empty",
        is_judgeable("crates/clio-types/src/lib.rs"),
        True,
    )
    check(
        "a crate root with no executable code is exempt by source",
        has_no_executable_code("crates/clio-types/src/lib.rs"),
        True,
    )
    # The matcher, on each shape that must and must not disqualify a file. The
    # `fn` token is matched rather than a whole line, so a body is still seen
    # when a visibility abuts `fn` or a newline separates `fn` from its name.
    for case_name, source, must_flag in (
        ("a plain function disqualifies", "pub fn thing() {}\n", True),
        ("a private function disqualifies", "fn helper() {}\n", True),
        ("a const fn disqualifies", "const fn make() -> u8 { 1 }\n", True),
        ("an async fn disqualifies", "async fn go() {}\n", True),
        ("an unsafe fn disqualifies", "unsafe fn raw() {}\n", True),
        ("a visibility with no space before fn disqualifies", "pub(crate)fn thing() {}\n", True),
        ("a newline between fn and its name disqualifies", "fn\nthing() {}\n", True),
        ("a visibility then a newline then fn disqualifies", "pub(crate)\nfn thing() {}\n", True),
        ("a bodyless declaration with no space does not", 'extern "C"{fn c_fn();}\n', False),
        ("a type alias to a fn pointer does not", "type F = fn(u8) -> u8;\n", False),
        ("a method in an impl disqualifies", "impl A {\n    pub fn call(&self) {}\n}\n", True),
        ("a foreign fn declaration does not", 'extern "C" {\n    fn c_fn();\n}\n', False),
        ("a trait method declaration does not", "trait Store: Send {\n    fn go(&self) -> u8;\n}\n", False),
        ("a multi-line trait method declaration does not", "trait Store {\n    fn revise(\n        &self,\n        bank: &str,\n    ) -> Result<(), Error>;\n}\n", False),
        ("a trait method with a default body disqualifies", "trait Store {\n    fn go(&self) -> u8 {\n        1\n    }\n}\n", True),
        ("a declaration whose parameter holds an array does not", "trait Store {\n    fn go(&self, buf: [u8; 4]);\n}\n", False),
        ("a body after a comment holding a semicolon disqualifies", "fn go() /* ; */ {\n    1\n}\n", True),
        ("a macro_rules definition disqualifies", "macro_rules! go {\n    () => {};\n}\n", True),
        ("module declarations only do not", "mod a;\npub mod b;\n", False),
        ("use statements only do not", "use std::fmt;\npub use a::B;\n", False),
        ("type aliases only do not", "pub type R = ();\n", False),
        ("const and static values do not", "pub const N: usize = 2;\nstatic S: &str = \"\";\n", False),
        ("a mention of fn in a comment does not", "// fn never_defined() {}\n", False),
        ("a mention of fn in a doc comment does not", "/// calls fn thing() elsewhere\n", False),
        ("fn inside a string does not", 'const S: &str = "fn thing() {}";\n', False),
        ("an empty file does not", "", False),
    ):
        check(case_name, bool(offenders(source)), must_flag)
    check(
        "a crate root that does define functions stays judged",
        is_judgeable("crates/clio/src/lib.rs"),
        True,
    )
    # A deleted file is a changed file with nothing left to measure. Left in the
    # judged list it was reported as a file the run failed to measure, so a change
    # that only retired a file failed a gate that had nothing to say about it.
    check("a file in the tree is present", is_present("crates/clio-types/src/civil.rs"), True)
    check(
        "a file the change deleted is not present",
        is_present("crates/clio-types/src/retired.rs"),
        False,
    )
    # A count alone is not a diagnosis. Both scopes name every file they could not
    # pass, because a reader holding only "22 below floor" has to diff the printed
    # pass list against the judged list to learn which 22 those were.
    verdict_log = io.StringIO()
    report_verdicts(
        ["crates/a/src/low.rs lines 71.00% (floor 80.00)"],
        ["crates/a/src/gone.rs"],
        verdict_log,
    )
    printed = verdict_log.getvalue()
    check("a failing run names the file below the floor", "crates/a/src/low.rs" in printed, True)
    check("a failing run names the file it could not measure", "gone.rs" in printed, True)

    # The selection is `make quick`'s: each changed file's own test modules, all in
    # ONE run over only the packages that hold them. The verdict reads only the
    # changed files that are not exempt.
    names = ("select", "is_present", "is_measureable", "owned_terms", "exemption", "run_tests", "run_coverage", "load_report", "report_rows")
    saved = {name: globals()[name] for name in names}
    saved_lookup = owned.owned_selection
    real_stderr = sys.stderr
    calls: list = []
    rel = "crates/a/src/x.rs"
    term = Term("a", modules=("x::tests",))
    try:
        globals()["select"] = lambda base, everything: (["a", "dependent"], [rel], [rel])
        globals()["is_present"] = lambda path: True
        globals()["is_measureable"] = lambda path: True
        globals()["exemption"] = lambda path: None
        globals()["owned_terms"] = lambda files: ([], [], [])
        owned.owned_selection = lambda path: owned.OwnedSelection(path, None, [])
        sys.stderr = io.StringIO()
        check("test-fast with no selected test runs nothing and passes", main(["--tests-only"]), EXIT_OK)
        check("coverage-fast with no selected test still judges", main([]), EXIT_FAIL)
        check("a file with no test module of its own fails", f"FAIL {rel} owns no test module" in sys.stderr.getvalue(), True)

        globals()["owned_terms"] = lambda files: ([term], ["--lib", "--bins"], [])
        globals()["run_tests"] = lambda packages, expression, targets: calls.append((packages, targets)) or 4
        check("test-fast returns the test run's own exit code", main(["--tests-only"]), 4)
        check("test-fast runs only the packages that hold selected tests", calls, [(["a"], ["--lib", "--bins"])])

        calls.clear()
        globals()["run_coverage"] = lambda packages, expression, targets: calls.append(packages)
        globals()["load_report"] = lambda path: {}
        owned.owned_selection = lambda path: owned.OwnedSelection(path, None, ["x::tests"])
        for pct, want in ((79.99, EXIT_FAIL), (80.0, EXIT_OK)):
            globals()["report_rows"] = lambda doc, pct=pct: {rel: CoverageRow(pct, 0.0, regions_pct=0.0)}
            check(f"coverage-fast at {pct}% lines", main([]), want)
        check("coverage-fast runs every selected test in one instrumented run", calls, [["a"], ["a"]])
        globals()["report_rows"] = lambda doc: {}
        check("a judged file the report does not hold fails", main([]), EXIT_FAIL)
        check("the missing file is named", f"FAIL {rel} is not in the report" in sys.stderr.getvalue(), True)
    finally:
        sys.stderr = real_stderr
        owned.owned_selection = saved_lookup
        for name, value in saved.items():
            globals()[name] = value

    # The files the verdict never judges.
    check("a crate's tests directory is exempt", exemption("crates/clio-mcp/tests/mcp_principal_binding.rs"), "test file")
    check("an examples directory is exempt", exemption("crates/clio/examples/demo.rs"), "example")
    check("a benches directory is exempt", exemption("crates/clio/benches/parse.rs"), "benchmark")
    check("a build script is exempt", exemption("crates/clio/build.rs"), "build script")
    check(
        "a report row carries its region percentage",
        report_rows({"data": [{"files": [{
            "filename": str(ROOT / "crates/a/src/x.rs"),
            "summary": {
                "lines": {"percent": 90.0, "count": 10, "covered": 9},
                "functions": {"percent": 50.0, "count": 2, "covered": 1},
                "regions": {"percent": 70.0, "count": 5, "covered": 3},
            },
        }]}]})["crates/a/src/x.rs"].regions_pct,
        70.0,
    )
    check(
        "declarations and re-exports only",
        declarations_only("//! doc\n#[cfg(feature = \"x\")]\npub mod a;\nmod b;\npub(crate) use a::{B, C};\nuse std::fmt;\n"),
        True,
    )
    check("a function is code", declarations_only("pub mod a;\nfn f() {}\n"), False)
    check("a type is code", declarations_only("pub struct S;\n"), False)

    # The two halves of the tool's exclusion, pinned separately. The suffix half
    # alone let every integration test be judged, and llvm-cov can never report
    # one, so each such change failed a gate it had already passed on coverage.
    check(
        "an integration test is not measured",
        is_measureable("crates/clio-mcp/tests/mcp_principal_binding.rs"),
        False,
    )
    check(
        "a module under a tests directory is not measured",
        is_measureable("crates/clio-mcp/tests/binding_support/mod.rs"),
        False,
    )
    check(
        "a tests directory inside src is not measured either",
        is_measureable("crates/clio-store/src/tests/mod.rs"),
        False,
    )
    check(
        "an examples or benches directory is not measured",
        (
            is_measureable("crates/clio/examples/demo.rs"),
            is_measureable("crates/clio/benches/parse.rs"),
        ),
        (False, False),
    )
    check(
        "a production file whose name merely contains tests is measured",
        (
            is_measureable("crates/clio/src/tests_helper.rs"),
            is_measureable("crates/clio/src/latest.rs"),
        ),
        (True, True),
    )
    # No file name exempts a file. The tool's suffix rule is not mirrored here:
    # every name below is measured, and only a path or a test gate exempts one.
    check(
        "no tests-suffixed name is exempt by its name",
        (
            is_measureable("crates/clio/src/tests.rs"),
            is_measureable("crates/clio/src/word_tests.rs"),
            is_measureable("crates/clio/src/word-tests.rs"),
            is_measureable("crates/clio/src/a_tests.rs"),
            is_measureable("crates/clio/src/_tests.rs"),
            is_measureable("crates/clio/src/mytests.rs"),
        ),
        (True, True, True, True, True, True),
    )

    # Untracked files are read with `ls-files`, not `status`, and the reason is
    # the one case `status` cannot express. A wholly untracked directory collapses
    # to a single entry ending in a slash, so a real git repository is the only
    # way to pin this: a mock would agree with whichever implementation it was
    # written against. The stub below reproduces exactly what git prints.
    original_git = globals()["git"]
    original_merge_base_for_untracked = globals()["merge_base"]
    try:
        globals()["merge_base"] = lambda base: "0" * 40

        def fake_git(*args: str) -> str:
            if args[:2] == ("ls-files", "--others"):
                # What `ls-files -z` prints for the same tree `status` collapses
                # into one entry: a path per file, NUL-separated.
                return "crates/x/src/lib.rs\0crates/x/Cargo.toml\0"
            return ""

        globals()["git"] = fake_git
        untracked = changed_files("master")
        every_type = changed_paths("master")
    finally:
        globals()["git"] = original_git
        globals()["merge_base"] = original_merge_base_for_untracked
    check(
        "a Rust file under a wholly untracked directory is seen",
        untracked,
        ["crates/x/src/lib.rs"],
    )
    check(
        "changed_paths keeps every file type and changed_files only the Rust ones",
        every_type,
        ["crates/x/Cargo.toml", "crates/x/src/lib.rs"],
    )

    # The tests-only mode must be a plain nextest run with no instrumentation
    # flags and no report, because the whole point of it is to skip the two
    # expensive steps. A stray --json or llvm-cov here would silently turn the
    # fast loop back into the slow one.
    recorded: dict[str, list[str]] = {}

    class _Done:
        returncode = 0

    original_run = globals()["subprocess"].run

    def fake_run(cmd, **kw):
        recorded["cmd"] = list(cmd)
        return _Done()

    try:
        globals()["subprocess"].run = fake_run
        code = run_tests(["a", "b"])
    finally:
        globals()["subprocess"].run = original_run
    cmd = recorded["cmd"]
    check("tests-only runs nextest", cmd[:3], ["cargo", "nextest", "run"])
    check("tests-only passes both packages", cmd[3:7], ["-p", "a", "-p", "b"])
    check("tests-only is not instrumented", "llvm-cov" in cmd, False)
    check("tests-only exports no report", "--json" in cmd, False)
    check("tests-only keeps the warm build", "--no-clean" in cmd, False)
    check("tests-only returns nextest's exit code", code, 0)
    check("tests-only uses the ci profile", "--profile" in cmd and "ci" in cmd, True)
    check("the crate scope sends no filter", "-E" in cmd, False)

    # The file scope sends one filter expression. Two invocations would run the
    # overlap twice, so the count of `-E` flags is asserted rather than assumed.
    try:
        globals()["subprocess"].run = fake_run
        run_tests(["a"], "test(/^foo::tests(::|$)/)")
    finally:
        globals()["subprocess"].run = original_run
    cmd = recorded["cmd"]
    check("the file scope sends exactly one filter", cmd.count("-E"), 1)
    check("the filter follows its expression", cmd[cmd.index("-E") + 1], "test(/^foo::tests(::|$)/)")

    # A filter expression built from real module paths must be anchored, because
    # nextest's `test()` is an unanchored substring match: `foo::tests` would
    # otherwise also select `other::foo::tests`.
    check(
        "an expression anchors the module path at both ends",
        owned.filter_expression(["foo::tests"]) == "test(/^foo::tests(::|$)/)",
        True,
    )
    union = owned.filter_expression(["a::tests", "b::tests"])
    check("a union is wrapped in one expression", union.startswith("(") and union.endswith(")"), True)
    check("a union has one branch per module", union.count("test("), 2)
    check("no modules yields no expression", owned.filter_expression([]), "")

    # The one rule, in the one judge both scopes call. A file below the floor
    # fails whatever its function coverage says, which is the whole of what the
    # function half of a row is for: it is printed, and nothing compares it.
    thin_offenders, thin_missing = judge(
        {"crates/a/src/thin.rs": CoverageRow(71.0, 100.0)},
        ["crates/a/src/thin.rs"],
    )
    check("a file at 71 lines fails at 100 functions", len(thin_offenders), 1)
    check("a file at 71 lines is named", "thin.rs" in thin_offenders[0], True)
    check("nothing is unreported when the report holds the file", thin_missing, [])
    fine_offenders, _ = judge(
        {"crates/a/src/fine.rs": CoverageRow(91.8, 40.0)}, ["crates/a/src/fine.rs"]
    )
    check("a file at 91.8 lines passes at 40 functions", fine_offenders, [])
    _, fixed_missing = judge({}, ["crates/a/src/gone.rs"])
    check("an unmeasured file is reported, not passed", fixed_missing, ["crates/a/src/gone.rs"])
    check(
        "the floor is the one number in this file, and it is 80",
        LINE_FLOOR,
        80.0,
    )
    check(
        "a file one line under the floor still fails, so nothing rounds in its favour",
        len(judge({"crates/a/src/x.rs": CoverageRow(79.99, 100.0)}, ["crates/a/src/x.rs"])[0]),
        1,
    )
    check(
        "a file exactly at the floor passes, and nothing else is consulted",
        judge({"crates/a/src/x.rs": CoverageRow(80.0, 0.0)}, ["crates/a/src/x.rs"])[0],
        [],
    )
    return printed,
