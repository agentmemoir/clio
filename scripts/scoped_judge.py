"""Judging the coverage report: rows, renames, the file scope, and the verdict against the line floor."""

from __future__ import annotations

import json
import re
from pathlib import Path, PurePosixPath
from typing import Any, Collection, Iterable, NamedTuple, Sequence

from scoped_core import CoverageRow, LINE_FLOOR, ROOT, RunError, owned
import owned_scanner as scanner
from scoped_select import git, merge_base


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
            regions = summary.get("regions", {})
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
                float(regions["percent"]) if isinstance(regions.get("percent"), (int, float)) else None,
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


_ATTRIBUTE = re.compile(r"#!?\[[^\]]*\]")
_DECLARATION = re.compile(r"(?:pub(?:\([^)]*\))?\s+)?(?:mod\s+\w+|use\s[^;]*)\s*;")


def declarations_only(source: str) -> bool:
    """True when the code is only `mod x;`, `use ...;` and `pub use ...;`, with attributes and comments."""
    code = _ATTRIBUTE.sub(" ", scanner.strip_comments(source))
    return not _DECLARATION.sub(" ", code).strip()


def is_test_module(rel: str) -> bool:
    """True when the file, or a module above it, is declared behind a test gate."""
    module_of = owned.walk_crate(owned.crate_of(rel))
    path_of = {module: path for path, module in module_of.items()}
    target = (ROOT / rel).resolve()
    module = next((m for p, m in module_of.items() if p.resolve() == target), None)
    while module:
        parent, _, name = module.rpartition("::")
        source = scanner.read_text(path_of[parent])
        if any(d.name == name and d.gated for d in scanner.declarations(source, path_of[parent].parent)):
            return True
        module = parent
    return False


def _directly_under(rel: str, name: str) -> bool:
    """True when the file sits under a `name/` directory at the top of its crate."""
    try:
        inside = (ROOT / rel).resolve().relative_to(owned.crate_of(rel))
    except owned.ResolutionError:
        return False
    return inside.parts[:1] == (name,)


def exemption(rel: str) -> str | None:
    """Why a changed file is never judged, or None when it is judged.

    A file is exempt by its place (a crate's tests, examples or benches directory),
    by being build.rs, by holding only declarations, or by being declared behind a
    test gate. No file name exempts a file.
    """
    if _directly_under(rel, "tests"):
        return "test file"
    if _directly_under(rel, "examples"):
        return "example"
    if _directly_under(rel, "benches"):
        return "benchmark"
    if PurePosixPath(rel).name == "build.rs":
        return "build script"
    if declarations_only((ROOT / rel).read_text(encoding="utf-8")):
        return "module declarations and re-exports only"
    if is_test_module(rel):
        return "declared test module"
    return None


def verdicts(rows: dict[str, CoverageRow], files: Sequence[str], floor: float = LINE_FLOOR) -> list[str]:
    """Every failure over the judged files: no test module of its own, not in the report, or below the floor."""
    untested = [rel for rel in files if owned.owned_selection(rel).is_empty]
    below, missing = judge(rows, [rel for rel in files if rel not in untested], floor)
    return (
        [f"{rel} owns no test module" for rel in untested]
        + [f"{rel} is not in the report" for rel in missing]
        + below
    )


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
