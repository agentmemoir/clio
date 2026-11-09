#!/usr/bin/env python3
"""Choose the one pack shape the build ships, from a probe and two measurements.

The build emits exactly one pack shape, so the choice is made once, in one
place, and recorded once. This module measures both candidate packs before it
chooses anything: the reference candidate keeps its local `$ref` and `$defs`,
the flattened candidate resolves every reference in place, and both are measured
with the same canonical tools serialization and the same digest the budget gate
uses, by the pinned offline tokenizer.

The choice reads one input, the probe receipt: a probe that passed ships the
reference candidate, and a probe that failed or could not run ships the
flattened one. An unrun probe never counts as a pass, because an unobserved
client is not evidence of support.

The record this writes, `benchmarks/mcp_token_budget/pack_shape.json`, is that
one record: the receipt, both measurements, and the shape the build actually
emits with its digest. The computed shape is refused unless it equals the shape
the crate's own constant ships, and an existing record that names a different
shipped shape is refused instead of being rewritten, so the record cannot drift
away from the build by accident.

Every error is one line prefixed ``pack-shape:`` and exits 2; nothing is written
on a failure, because the record is built in memory and written once.
"""

import argparse
import json
import re
import sys
from pathlib import Path

import budget_check
import probe_pack_shape
from measure import _parse_pack
from offline_tokenizer import BudgetError, load

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]

RECORD_FORMAT = "clio_mcp_pack_shape"
RECORD_FORMAT_VERSION = 1
RECORD_ID = "pack-shape-v1"

#: The record's repo-relative path; never absolute, never host-specific.
RECORD_PATH = "benchmarks/mcp_token_budget/pack_shape.json"
RECORD_FILE = HERE / "pack_shape.json"

#: The crate's own one-shape constant, read so the record cannot disagree with it.
CRATE_SHAPE_PATH = "crates/clio-mcp/src/schema_shape.rs"
CRATE_SHAPE_FILE = ROOT / CRATE_SHAPE_PATH
CRATE_SHAPE_RE = re.compile(
    r"pub const SHIPPED_PACK_SHAPE: PackShape = PackShape::(\w+);"
)

SHAPE_REFERENCE = "reference"
SHAPE_FLATTENED = "flattened"
SHAPES = (SHAPE_REFERENCE, SHAPE_FLATTENED)

#: The two candidates, in record order: the reference shape first, the shipped
#: flattened shape second. Each path holds a complete pack document.
CANDIDATE_PATHS = (
    (SHAPE_REFERENCE, "crates/clio-mcp/tests/fixtures/schema_pack_reference.json"),
    (SHAPE_FLATTENED, "crates/clio-mcp/tests/fixtures/schema_pack.json"),
)

PROBE_PASSED = probe_pack_shape.STATUS_PASSED


class PackShapeError(ValueError):
    """Raised when the shape choice cannot be made or recorded safely."""


def _fail(message):
    raise PackShapeError(f"pack-shape: {message}")


def choose(probe_status) -> str:
    """Return the shape to ship for ``probe_status``; only a pass ships reference."""
    if probe_status == PROBE_PASSED:
        return SHAPE_REFERENCE
    if probe_status in probe_pack_shape.STATUSES:
        return SHAPE_FLATTENED
    _fail(f"unknown probe status: {probe_status!r}")


def crate_shape(path=CRATE_SHAPE_FILE) -> str:
    """Return the shape the crate's own constant ships, read from its source."""
    try:
        text = Path(path).read_text(encoding="utf-8")
    except OSError as exc:
        _fail(f"cannot read the crate's shape constant: {exc}")
    match = CRATE_SHAPE_RE.search(text)
    if match is None:
        _fail(f"cannot read SHIPPED_PACK_SHAPE from {CRATE_SHAPE_PATH}")
    name = match.group(1).lower()
    if name not in SHAPES:
        _fail(f"SHIPPED_PACK_SHAPE names an unknown shape: {match.group(1)}")
    return name


def measure(path, tokenizer) -> dict:
    """Return one candidate's measurement, keyed the way the budget gate keys it."""
    try:
        raw = Path(path).read_bytes()
    except OSError as exc:
        _fail(f"cannot read candidate pack {path}: {exc}")
    pack = _parse_pack(raw)
    return budget_check.measure_candidate(pack, tokenizer)


def measure_candidates(tokenizer) -> dict:
    """Return both candidate measurements, keyed by shape, measured before any choice."""
    measured = {}
    for shape, relative in CANDIDATE_PATHS:
        measured[shape] = {
            "shape": shape,
            "path": relative,
            **measure(ROOT / relative, tokenizer),
        }
    return measured


def build_record(receipt, tokenizer=None, crate=None) -> dict:
    """Return the one shape record: the receipt, both measurements, the shipped shape.

    Both candidates are measured first and the choice is read from the receipt
    afterwards, because the measurement is what the record reports whichever way
    the probe goes. A computed shape the crate does not ship is refused here, so
    a record can never name a shape the build does not emit.
    """
    measured = measure_candidates(load() if tokenizer is None else tokenizer)
    shipped = choose(receipt["status"])
    expected = crate_shape() if crate is None else crate
    if shipped != expected:
        _fail(
            f"the probe chooses {shipped!r} but the crate ships {expected!r}; "
            "the record would name a shape the build does not emit"
        )
    return {
        "format": RECORD_FORMAT,
        "format_version": RECORD_FORMAT_VERSION,
        "record_id": RECORD_ID,
        "probe": receipt,
        "candidates": [measured[shape] for shape, _ in CANDIDATE_PATHS],
        "shipped": {
            "shape": shipped,
            "path": measured[shipped]["path"],
            "sha256": measured[shipped]["sha256"],
        },
    }


def existing_shape(path=RECORD_FILE):
    """Return the shape an existing record names as shipped, or None."""
    path = Path(path)
    if not path.exists():
        return None
    try:
        record = json.loads(path.read_text(encoding="utf-8"))
    except OSError as exc:
        _fail(f"cannot read the existing record: {exc}")
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        _fail(f"the existing record is not valid JSON: {exc}")
    shipped = record.get("shipped") if isinstance(record, dict) else None
    shape = shipped.get("shape") if isinstance(shipped, dict) else None
    if not isinstance(shape, str):
        _fail("the existing record names no shipped shape")
    return shape


def write_record(record, path=RECORD_FILE) -> None:
    """Write ``record`` once, refusing to rewrite a record of another shipped shape."""
    previous = existing_shape(path)
    if previous is not None and previous != record["shipped"]["shape"]:
        _fail(
            f"the existing record ships {previous!r} and this run ships "
            f"{record['shipped']['shape']!r}; refusing to rewrite it"
        )
    text = json.dumps(record, ensure_ascii=False, indent=2) + "\n"
    Path(path).write_text(text, encoding="utf-8")


def main(argv=None) -> int:
    """Measure both candidates, choose one shape, and write the one record."""
    parser = argparse.ArgumentParser(
        prog="pack_shape.py",
        description="Choose the one pack shape the build ships and record why.",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="print the record instead of writing it",
    )
    parser.add_argument(
        "--out",
        metavar="PATH",
        default=None,
        help=f"record to write; defaults to {RECORD_PATH}",
    )
    parser.add_argument(
        "--baseline",
        metavar="PATH",
        default=None,
        help="baseline capture record the probe reads",
    )
    args = parser.parse_args(argv)

    try:
        receipt = probe_pack_shape.probe(args.baseline or probe_pack_shape.BASELINE_FILE)
        record = build_record(receipt)
        if args.json:
            sys.stdout.write(json.dumps(record, ensure_ascii=False, indent=2) + "\n")
        else:
            write_record(record, args.out or RECORD_FILE)
            sys.stdout.write(
                f"pack-shape: probe {receipt['status']}, shipped "
                f"{record['shipped']['shape']} ({record['shipped']['sha256']}) "
                f"recorded in {RECORD_PATH}\n"
            )
    except (PackShapeError, BudgetError, probe_pack_shape.ProbeError) as exc:
        sys.stderr.write(f"{exc}\n")
        return 2

    sys.stdout.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())
