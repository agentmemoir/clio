#!/usr/bin/env python3
"""Fail the build when the MCP tool surface grows past its reviewed budget.

The check measures one schema pack the same way the frozen baseline was measured:
the tools array is canonicalized (sorted keys, no whitespace, UTF-8) and only
those bytes are described, by their length and by their cl100k_base token count.
Both arms are then compared with the matching row of the frozen baseline, which
is pinned here by digest: a baseline file whose bytes moved is refused instead of
being compared against a number nobody reviewed.

A candidate that stays inside the baseline passes with no allowance. A candidate
that grows must be matched by exactly one growth-allowance record naming its own
digest, and both measured values must fit that record's independent ceilings.

The two arms stay independent when a record applies: each arm reads only its own
measure's problems, so a breach of the byte ceiling fails the bytes arm and names
``bytes`` while the tokens arm still passes inside its own ceiling, and the mirror
case holds too. A message never reaches both arms and never appears twice.
Anything else fails and names the measure, the numbers, and the digest.

The always-loaded skill entry point is compared the same way when ``--skill``
names a file, against its own frozen rows and with no allowance: an allowance
names a pack digest, so it says nothing about a skill file. When no path is
given the report carries a ``skill`` object marked ``not-measured`` that still
shows both frozen skill rows, read through the same named-row lookup, instead of
reporting zero. An explicitly empty ``--skill`` path is an unusable input and
exits 2, because it is a wiring mistake, not a decision to skip the arm.

The activated skill body is always measured, against the one frozen row that
carries its token count and the factor that count is multiplied by. The bound is
token growth alone, so a body whose bytes or digest moved without growing its
count passes and the two digests are reported side by side.

The preset targets are reported beside the arms. They are not enforced yet, so a
target miss appears in the report and leaves the exit code alone; a targets file
that cannot be trusted is an unusable input, not a silent skip.

The whole report is built in memory and written once, so a failure can never
leave half a document behind. An unusable input exits 2 with
``mcp-budget-check: <message>`` on stderr; a measured verdict of fail exits 1 and
still prints the report, so the failure reason reaches the build log.
"""

import argparse
import hashlib
import json
import sys
from pathlib import Path

import activated_budget
import baseline_provenance
import budget_allowances
import budget_targets
from measure import _parse_pack, _read_pack
from offline_tokenizer import BudgetError, load

HERE = Path(__file__).resolve().parent

OUTPUT_FORMAT = "clio_mcp_budget_check"
OUTPUT_FORMAT_VERSION = 1

#: Digest of the committed baseline file. An edit or an append changes this value.
PINNED_BASELINE_SHA256 = budget_targets.PINNED_BASELINE_SHA256

#: Repo-relative paths reported in the output; never absolute, never host-specific.
BASELINE_PATH = "benchmarks/mcp_token_budget/baseline/frozen-v1.json"
BASELINE_FILE = HERE / "baseline" / "frozen-v1.json"
ALLOWANCES_PATH = HERE / "allowances" / "frozen-v1.json"
TARGETS_FILE = HERE / "targets" / "frozen-v1.json"

BASELINE_ROW_BYTES = "client/synthetic-eager-client/turn/1/bytes"
BASELINE_ROW_TOKENS = "client/synthetic-eager-client/turn/1/tokens"

#: The two independent arms, in report order.
ARMS = (("bytes", BASELINE_ROW_BYTES), ("tokens", BASELINE_ROW_TOKENS))

#: The always-loaded skill file's own rows; a pack allowance never covers them.
SKILL_ROW_BYTES = "retired_skill_entry_point/bytes"
SKILL_ROW_TOKENS = "retired_skill_entry_point/tokens"
SKILL_ARMS = (("bytes", SKILL_ROW_BYTES), ("tokens", SKILL_ROW_TOKENS))

NO_SKILL_PATH = "no --skill path given"
SKILL_NOT_MEASURED = "not-measured"

ALLOWANCE_SUMMARY_KEYS = ("decision_id", "date", "base_commit", "max_bytes", "max_tokens")


def load_baseline(path=BASELINE_FILE):
    """Return ``(record, digest)`` for the baseline file, refusing a moved baseline."""
    try:
        raw = Path(path).read_bytes()
    except OSError as exc:
        raise BudgetError(f"budget: cannot read baseline: {exc}") from None
    digest = hashlib.sha256(raw).hexdigest()
    if digest != PINNED_BASELINE_SHA256:
        raise BudgetError(
            f"budget: baseline digest is {digest}, expected the pinned "
            f"{PINNED_BASELINE_SHA256}"
        )
    try:
        record = json.loads(raw.decode("utf-8"))
    except UnicodeDecodeError as exc:
        raise BudgetError(f"budget: baseline is not valid UTF-8: {exc}") from None
    except json.JSONDecodeError as exc:
        raise BudgetError(f"budget: baseline is not valid JSON: {exc}") from None
    if not isinstance(record, dict):
        raise BudgetError(
            f"budget: baseline must be a JSON object, got {type(record).__name__}"
        )
    return record, digest


def _row_unit(row_id) -> str:
    return row_id.rsplit("/", 1)[-1]


def baseline_value(record, row_id) -> int:
    """Return the positive integer value of ``row_id`` in ``record``; never a default."""
    rows = record.get("rows")
    rows = rows if isinstance(rows, list) else []
    matches = [row for row in rows if isinstance(row, dict) and row.get("row") == row_id]
    if not matches:
        raise BudgetError(f"budget: missing baseline row: {row_id}")
    if len(matches) > 1:
        raise BudgetError(f"budget: duplicate baseline row: {row_id}")
    row = matches[0]
    unit = _row_unit(row_id)
    if row.get("unit") != unit:
        raise BudgetError(
            f"budget: baseline row {row_id} unit is {row.get('unit')!r}, expected {unit!r}"
        )
    value = row.get("value")
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise BudgetError(
            f"budget: baseline row {row_id} value is not a positive integer: {value!r}"
        )
    return value


def arms_from(record, digest) -> dict:
    """Return the resolved ceilings and digest of one baseline record."""
    arms = {"path": BASELINE_PATH, "sha256": digest}
    for measure, row_id in ARMS:
        arms[measure] = baseline_value(record, row_id)
    return arms


def baseline_arms(path=BASELINE_FILE) -> dict:
    """Return the resolved ceilings and digest of the frozen baseline."""
    record, digest = load_baseline(path)
    return arms_from(record, digest)


def baseline_rows(record) -> dict:
    """Return every named row value in ``record``, keyed by row id."""
    rows = record.get("rows")
    rows = rows if isinstance(rows, list) else []
    return {
        row["row"]: row["value"]
        for row in rows
        if isinstance(row, dict) and isinstance(row.get("row"), str)
    }


def measure_skill(path, tokenizer) -> dict:
    """Return the digest, raw byte length, and token count of the file at ``path``."""
    try:
        raw = Path(path).read_bytes()
    except OSError as exc:
        raise BudgetError(f"budget: cannot read skill file: {exc}") from None
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise BudgetError(f"budget: skill file is not valid UTF-8: {exc}") from None
    return {
        "sha256": hashlib.sha256(raw).hexdigest(),
        "bytes": len(raw),
        "tokens": tokenizer.count(text),
    }


def skill_row_values(record) -> dict:
    """Return the frozen skill row values, read by name so a missing row raises."""
    return {row_id: baseline_value(record, row_id) for _, row_id in SKILL_ARMS}


def skill_not_measured(rows) -> dict:
    """Return the ``skill`` object for a run that supplied no skill path."""
    return {
        "status": SKILL_NOT_MEASURED,
        "reason": NO_SKILL_PATH,
        "baseline_rows": rows,
    }


def check_skill(record, candidate) -> dict:
    """Compare one measured skill file with its frozen rows; it carries no allowance."""
    rows = skill_row_values(record)
    arms = []
    for measure, row_id in SKILL_ARMS:
        base_value = rows[row_id]
        candidate_value = candidate[measure]
        delta = candidate_value - base_value
        arm = {
            "measure": measure,
            "row": row_id,
            "baseline": base_value,
            "candidate": candidate_value,
            "allowance": None,
            "delta": delta,
            "verdict": "pass",
        }
        if delta > 0:
            arm["verdict"] = "fail"
            arm["failure"] = (
                f"{row_id}: candidate {candidate_value} exceeds baseline "
                f"{base_value} by {delta}"
            )
        arms.append(arm)
    failures = [arm["failure"] for arm in arms if "failure" in arm]
    return {
        "sha256": candidate["sha256"],
        "bytes": candidate["bytes"],
        "tokens": candidate["tokens"],
        "baseline_rows": rows,
        "arms": arms,
        "failures": failures,
        "verdict": "fail" if failures else "pass",
    }


def measure_candidate(pack, tokenizer) -> dict:
    """Return the digest, canonical byte length, and token count of ``pack``."""
    canonical = baseline_provenance.canonical_tools(pack["tools"])
    return {
        "sha256": hashlib.sha256(canonical).hexdigest(),
        "bytes": len(canonical),
        "tokens": tokenizer.count(canonical.decode("utf-8")),
    }


def _allowance_records(allowances, baseline_digest) -> list:
    """Return the records from a validated document or a bare list, checking its target."""
    if isinstance(allowances, dict):
        declared = allowances.get("baseline")
        declared = declared.get("sha256") if isinstance(declared, dict) else None
        if declared != baseline_digest:
            raise BudgetError(
                f"budget: allowance record targets a different baseline: {declared}"
            )
        allowances = allowances.get("records")
    if not isinstance(allowances, list):
        raise BudgetError(
            f"budget: allowance records must be a list, got {type(allowances).__name__}"
        )
    return allowances


def _summary(record) -> dict:
    return {key: record[key] for key in ALLOWANCE_SUMMARY_KEYS}


def _arm(measure, baseline, candidate, records) -> dict:
    """Return one arm's comparison: pass inside the ceiling or inside its allowance."""
    base_value = baseline[measure]
    candidate_value = candidate[measure]
    delta = candidate_value - base_value
    arm = {
        "measure": measure,
        "baseline": base_value,
        "candidate": candidate_value,
        "allowance": None,
        "delta": delta,
        "verdict": "pass",
    }
    if delta <= 0:
        return arm
    record = budget_allowances.applicable(records, candidate["sha256"])
    if record is None:
        arm["verdict"] = "fail"
        arm["failure"] = (
            f"{measure}: candidate {candidate_value} exceeds baseline {base_value} by "
            f"{delta} with no allowance record for digest {candidate['sha256']}"
        )
        return arm
    _, problems = budget_allowances.within(
        record, candidate["bytes"], candidate["tokens"]
    )
    # Only this arm's own measure decides this arm; the other measure is its own arm's.
    own = problems[measure]
    if own:
        arm["verdict"] = "fail"
        arm["failure"] = "; ".join(own)
        return arm
    arm["allowance"] = _summary(record)
    return arm


def check_pack(
    baseline, candidate, allowances, skill=None, activated=None, targets=None,
    skill_rows=None,
) -> dict:
    """Return the report for one measured candidate against ``baseline``.

    ``baseline`` holds the resolved ceilings, the baseline file digest, and the
    repo-relative path. ``allowances`` is a validated allowance document or a bare
    record list; a document's declared baseline digest must match ``baseline``.
    ``skill`` is the comparison returned by `check_skill`, ``activated`` the one
    returned by `activated_budget.check`, and ``targets`` the one returned by
    `budget_targets.compare`; the preset targets stay unenforced, and the
    activated body is reported even when its digest differs. When no skill was
    measured the ``skill`` entry still names both frozen rows through
    ``skill_rows`` — or, when the caller passes none, through the frozen record
    itself — so a reader never mistakes an unmeasured arm for a zero cost.
    """
    digest = baseline.get("sha256")
    records = _allowance_records(allowances, digest)
    arms = [_arm(measure, baseline, candidate, records) for measure, _ in ARMS]
    if skill is None:
        if skill_rows is None:
            skill_rows = skill_row_values(load_baseline()[0])
        skill = skill_not_measured(skill_rows)
    if activated is None:
        activated = activated_budget.not_measured(load_baseline()[0])
    failures = [arm["failure"] for arm in arms if "failure" in arm]
    failures.extend(skill.get("failures", []))
    failures.extend(activated["failures"])
    if targets is not None:
        failures.extend(targets["failures"])
    return {
        "baseline": {"path": baseline.get("path") or BASELINE_PATH, "sha256": digest},
        "candidate": {
            "sha256": candidate["sha256"],
            "bytes": candidate["bytes"],
            "tokens": candidate["tokens"],
        },
        "allowance": next((arm["allowance"] for arm in arms if arm["allowance"]), None),
        "arms": arms,
        "skill": skill,
        "activated": activated,
        "targets": targets,
        "failures": failures,
        "verdict": "fail" if failures else "pass",
    }


def render(report) -> str:
    """Return ``report`` as the one JSON document the check prints."""
    document = {
        "format": OUTPUT_FORMAT,
        "format_version": OUTPUT_FORMAT_VERSION,
        "baseline": report["baseline"],
        "candidate": report["candidate"],
        "allowance": report["allowance"],
        "arms": report["arms"],
        "skill": report["skill"],
        "activated": report["activated"],
        "targets": report["targets"],
        "failures": report["failures"],
        "verdict": report["verdict"],
    }
    return json.dumps(document, ensure_ascii=False, indent=2) + "\n"


class _Parser(argparse.ArgumentParser):
    """Argument parser that reports failures in this check's stderr format."""

    def error(self, message):
        sys.stderr.write(f"mcp-budget-check: {message}\n")
        raise SystemExit(2)


def main(argv=None) -> int:
    """Check ``--pack`` and print the report; return the process exit code."""
    parser = _Parser(
        prog="budget_check.py",
        description="Check a schema pack against the frozen MCP token budget.",
    )
    parser.add_argument(
        "--pack",
        metavar="PATH",
        default=None,
        help="pack file to measure; reads stdin when omitted",
    )
    parser.add_argument(
        "--allowances",
        metavar="PATH",
        default=None,
        help="growth-allowance record file; an absent file allows no growth",
    )
    parser.add_argument(
        "--skill",
        metavar="PATH",
        default=None,
        help="always-loaded skill file to compare; not measured when omitted",
    )
    parser.add_argument(
        "--activated-skill",
        metavar="PATH",
        default=None,
        help="activated skill body to compare; defaults to the shipped body",
    )
    args = parser.parse_args(argv)

    try:
        if args.skill == "":
            raise BudgetError("--skill path is empty")
        if args.activated_skill == "":
            raise BudgetError("--activated-skill path is empty")
        pack = _parse_pack(_read_pack(args.pack))
        record, digest = load_baseline()
        baseline = arms_from(record, digest)
        tokenizer = load()
        candidate = measure_candidate(pack, tokenizer)
        skill = None
        if args.skill:
            skill = check_skill(record, measure_skill(args.skill, tokenizer))
        body = args.activated_skill or activated_budget.DEFAULT_PATH
        activated = activated_budget.check(record, measure_skill(body, tokenizer))
        allowances = budget_allowances.load_or_empty(
            args.allowances or ALLOWANCES_PATH, digest
        )
        targets = budget_targets.compare(
            budget_targets.load(TARGETS_FILE), baseline_rows(record), candidate
        )
        report = check_pack(
            baseline,
            candidate,
            allowances,
            skill=skill,
            activated=activated,
            targets=targets,
            skill_rows=skill_row_values(record),
        )
        output = render(report)
    except (BudgetError, budget_allowances.AllowanceError, budget_targets.TargetError) as exc:
        sys.stderr.write(f"mcp-budget-check: {exc}\n")
        return 2

    sys.stdout.write(output)
    sys.stdout.flush()
    return 0 if report["verdict"] == "pass" else 1


if __name__ == "__main__":
    sys.exit(main())
