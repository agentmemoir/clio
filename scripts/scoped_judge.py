"""Judging the coverage report: rows, renames, the file scope, and the verdict against the line floor."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path, PurePosixPath
from typing import Any, Collection, Iterable, NamedTuple, Sequence

from scoped_core import COV_DIR, CoverageRow, LINE_FLOOR, ROOT, RunError, owned
from scoped_run import run_coverage
from scoped_select import git, merge_base, package_owners


# --- judging the report --------------------------------------------------------


def load_report(path: Path) -> dict[str, Any]:
    try:
        doc = json.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        raise RunError(f"cannot read report {path}: {error}") from error
    except ValueError as error:
        raise RunError(f"report is not valid JSON: {error}") from error
    if not isinstance(doc, dict) or not isinstance(doc.get("data"), list):
        raise RunError(f"{path} is not an llvm-cov JSON report")
    return doc


def report_rows(doc: dict[str, Any]) -> dict[str, CoverageRow]:
    """Per-file percentages and counts, keyed by a repo-relative path.

    The report spells paths absolutely and differently on every machine, so the
    key is the path relative to this checkout. That is what makes a row
    comparable to a changed-file list produced by git.

    A row whose percentages are missing is dropped, because there is nothing to
    judge. A row whose COUNTS are missing is kept, with the count left unknown,
    because a missing denominator is a shape this tool cannot judge and must say
    so about rather than invent.
    """
    rows: dict[str, CoverageRow] = {}
    for block in doc["data"]:
        for entry in block.get("files", []):
            filename = entry.get("filename")
            summary = entry.get("summary")
            if not isinstance(filename, str) or not isinstance(summary, dict):
                continue
            lines = summary.get("lines", {})
            functions = summary.get("functions", {})
            if not isinstance(lines.get("percent"), (int, float)):
                continue
            if not isinstance(functions.get("percent"), (int, float)):
                continue
            try:
                key = str(Path(filename).resolve().relative_to(ROOT))
            except ValueError:
                key = filename
            rows[key] = CoverageRow(
                float(lines["percent"]),
                float(functions["percent"]),
                _count(lines),
                _count(functions),
                _covered(lines),
                _covered(functions),
            )
    return rows


def _count(summary: dict[str, Any]) -> int | None:
    """The denominator in a summary block, or None when the report omits it.

    `None` and `0` are different answers and only one of them is allowed to be
    stored. A missing count means the report no longer says how much it
    measured, which the caller has to treat as a refusal; a zero means it
    measured nothing, which is a measurement.
    """
    value = summary.get("count")
    return value if isinstance(value, int) and not isinstance(value, bool) else None


def _covered(summary: dict[str, Any]) -> int | None:
    """The covered numerator in a summary block, or None when it is absent.

    This is the number a recorded count is compared against, so it has to come
    from the report rather than be recomputed from the percentage: the percentage
    is already rounded, and a numerator recovered from a rounded percentage is
    not the count the run measured.
    """
    value = summary.get("covered")
    return value if isinstance(value, int) and not isinstance(value, bool) else None


def renames(base: str) -> set[str]:
    """Named paths this change only moved, and did not edit.

    A pure rename produces no hunk, so the file's added-line set is empty and it
    would be reported the same way as one this change never touched. The two are
    different facts: nothing was added, because nothing about the content
    changed, and a reader looking at a coverage verdict for a file they only
    moved deserves to be told which of the two happened.

    This is a diagnostic, not a rule. Nothing about a rename changes a verdict:
    the file is judged on its own line coverage either way. It was removed by
    accident when the added-line gate it used to share a data path with went, and
    it is back because the information is about the diff rather than about
    coverage.
    """
    sha = merge_base(base)
    if sha is None:
        return set()
    moved: set[str] = set()
    # `-M` because rename detection is not implied by every diff call, and the
    # filter so only renames reach this parse.
    records = git(
        "diff", "--name-status", "-M", "--diff-filter=R", "-z", sha
    ).split("\0")
    for index in range(0, len(records) - 2, 3):
        status, _old, new = records[index : index + 3]
        if status.startswith("R"):
            moved.add(new)
    return moved


def file_scope_filter(files: Sequence[str]) -> tuple[str, list[str]]:
    """One filter expression naming every test `files` own, and the files owning none.

    Only valid for the test verdict, where a test either passed or did not. It is
    NOT valid for coverage: one run over several files' tests produces one merged
    report, and each file's row is then the union of all of their tests' coverage.
    Measured: `create.rs` reads 81.82% lines from its own tests and 92.12% when a
    second changed file's tests run alongside. `judge_file_scope` therefore runs
    one pass per file and this union is never used to measure.
    """
    selections = [owned.owned_selection(rel) for rel in files]
    unplanned = [s.file for s in selections if s.is_empty]
    modules: set[str] = set()
    binaries: set[str] = set()
    for selection in selections:
        modules.update(selection.modules)
        if selection.binary:
            binaries.add(selection.binary)
    if binaries:
        # Mixed targets: one branch per target, each naming only that target's
        # modules. A single `binary()` would name at most one target, and a
        # name-only union would pull in every target sharing a module path.
        branches = []
        for binary in sorted(binaries):
            own = {m for s in selections if s.binary == binary for m in s.modules}
            branches.append(owned.filter_expression(own, binary))
        for selection in selections:
            if selection.binary is None:
                branches.append(owned.filter_expression(selection.modules))
        return "(" + " | ".join(b for b in branches if b) + ")", unplanned
    return owned.filter_expression(modules), unplanned


def report_verdicts(
    offenders: Sequence[str],
    unreported: Sequence[str],
    stream: Any,
) -> None:
    """Name each file the report did not hold, then each file below the floor.

    One helper for both scopes because a count is not a diagnosis: a reader
    holding only "22 below floor" has to diff the printed pass list against the
    judged list to learn which files failed. The stream is a parameter because
    the two scopes already disagree about where these lines belong, and moving
    them would change output some reader depends on.
    """
    for rel in unreported:
        print(f"scoped-runner: NOT MEASURED {rel}", file=stream)
    for offender in offenders:
        print(f"scoped-runner: BELOW FLOOR {offender}", file=stream)


def judge_file_scope(
    packages: Sequence[str],
    files: Sequence[str],
    floor: float,
) -> tuple[list[str], list[str]]:
    """Measure and judge each changed file from that file's own tests, one run each.

    One run per file, not one run for the lot. A single instrumented pass over
    several files' tests writes one merged report, and in that report every
    file's row is the coverage contributed by all of them. That is not a small
    bias: measured here, `create.rs` reads 89.70% lines from its own tests and
    92.12% once a second changed file's tests join the run. Under that
    arrangement a file can be passed by another file's tests, which is the one
    thing a per-file measurement is for.

    The cost is one instrumented run per changed file rather than one per change.
    That is the right trade because a change usually touches a handful of files,
    and because correctness here is not recoverable from a faster wrong number.

    The judgement itself is `judge`, the same function the crate scope uses, at
    the same number. This scope differs in which tests produced the measurement
    and in nothing else.
    """
    offenders: list[str] = []
    unmeasured: list[str] = []
    for rel in files:
        selection = owned.owned_selection(rel)
        if selection.is_empty:
            # No test of its own means 0% coverage, which is a verdict rather than
            # a gap: the touch rule requires the file to be given tests.
            offenders.append(
                f"{rel} lines 0.00% (floor {floor:.2f}, no test module found)"
            )
            continue
        owners = package_owners(rel)
        expression = selection.expression()
        try:
            run_coverage(owners or packages, expression, report_path_for(rel))
            doc = load_report(report_path_for(rel))
        except RunError as error:
            raise RunError(f"{rel}: {error}") from error
        rows = report_rows(doc)
        below, missing = judge(rows, [rel], floor)
        offenders.extend(below)
        unmeasured.extend(missing)
        if not below and not missing:
            row = rows[rel]
            # Both metrics are printed, and only the lines one was judged: a report
            # that hid half of what it measured would be a different measurement.
            print(
                f"scoped-runner: {rel} lines {row.lines_pct:.2f}% "
                f"functions {row.functions_pct:.2f}% "
                f"from its own test module(s); judged on lines"
            )
    return offenders, unmeasured


def report_path_for(rel: str) -> Path:
    """A per-file report path, so one file's measurement cannot overwrite another's.

    A single shared path would let the last run's report stand in for the others,
    which is how a targeted number silently becomes a whole-crate one.
    """
    digest = hashlib.sha256(rel.encode("utf-8")).hexdigest()[:12]
    return COV_DIR / f"coverage-file-{digest}.json"


def judge(
    rows: dict[str, CoverageRow],
    files: Sequence[str],
    floor: float = LINE_FLOOR,
) -> tuple[list[str], list[str]]:
    """The coverage rule, in one place. Returns (offenders, unreported).

    Two lists, because two different things can be true of a changed file, and
    collapsing them would hide one:

    - `offenders`: the file is in the report and its line coverage is below
      `floor`. That is a failure, and it is the only failure this rule has.
    - `unreported`: the file is absent from the report. That is a gap and neither
      a pass nor a failure. Reporting it as green would be the exact lie this
      tool exists to avoid, so it is named and the run exits non-zero.

    The comparison is the line percentage against the one number in this
    repository, and nothing else is compared against anything. A file's function
    percentage is printed beside its line percentage and judged by no code path,
    because a percentage nobody compares is a measurement rather than a rule.

    There is deliberately no per-file override here and no argument to add one.
    A file that cannot reach the floor is a file whose tests do not exist yet,
    and the only honest way to record that is to write the tests, not to store
    its current number in a file the gate then reads as permission.
    """
    offenders: list[str] = []
    unreported: list[str] = []
    for rel in files:
        row = rows.get(rel)
        if row is None:
            unreported.append(rel)
            continue
        if row.lines_pct < floor:
            offenders.append(
                f"{rel} lines {row.lines_pct:.2f}% (floor {floor:.2f})"
            )
    return offenders, unreported
