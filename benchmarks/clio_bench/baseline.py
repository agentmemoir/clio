"""One command that produces a two-system baseline on the same pinned inputs.

The runner scores one system per invocation. This module runs the runner once
per system with the same suite, partition, seed, tokenizer, and judge, then
merges the per-system scorecards into one baseline artifact whose every number
names its tier and probe count. When the frontier key is absent the command
records the blocked gate instead, with both systems' fingerprints attached and
no score claimed.

No product code is involved: every system is driven through the existing
adapter registry and the existing runner.
"""

from __future__ import annotations

import argparse
import json
import sys
import tempfile
from pathlib import Path

from . import cli
from .fetch import load_pins
from .llm import frontier_key_present
from .report import (
    blocked_gate_record,
    render_settings_lines,
    render_suite_section,
    write_blocked_outputs,
)
from .runner import _DEFAULT_PINS, capture_fingerprints

SYSTEMS = ("clio", "mnemosyne")
DEFAULT_OUT = "benchmarks/reports/baseline.json"
DEFAULT_REPORT_MD = "benchmarks/reports/baseline.md"


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="clio-bench-baseline",
        description="Run both registered real systems on the same pinned inputs.",
    )
    parser.add_argument("--suite", default="both", choices=["both", "locomo", "longmemeval"])
    parser.add_argument("--partition", default="canary", choices=["train", "canary", "blind"])
    parser.add_argument("--judge", default="tier2", choices=["stub", "local", "tier2"])
    parser.add_argument("--generator", default="tier2", choices=["stub", "local", "tier2"])
    parser.add_argument("--max-probes", type=int, default=None)
    parser.add_argument("--max-instances", type=int, default=None)
    parser.add_argument("--token-budget", type=int, default=4096)
    parser.add_argument("--seed", type=int, default=None)
    parser.add_argument("--retrieve-limit", type=int, default=10)
    parser.add_argument("--clio-binary", default=None)
    parser.add_argument("--mnemosyne-binary", default=None)
    parser.add_argument("--clio-data-dir", default=None)
    parser.add_argument("--mnemosyne-data-dir", default=None)
    parser.add_argument("--cache-dir", default=None)
    parser.add_argument("--pins", default=str(_DEFAULT_PINS))
    parser.add_argument("--out", default=DEFAULT_OUT)
    parser.add_argument("--report-md", default=DEFAULT_REPORT_MD)
    return parser


def _binary_for(name: str, args) -> str | None:
    return args.clio_binary if name == "clio" else args.mnemosyne_binary


def _data_dir_for(name: str, args) -> str | None:
    return args.clio_data_dir if name == "clio" else args.mnemosyne_data_dir


def _runner_argv(name: str, args, out_path: Path, md_path: Path) -> list[str]:
    argv = [
        "--suite",
        args.suite,
        "--partition",
        args.partition,
        "--adapter",
        name,
        "--judge",
        args.judge,
        "--generator",
        args.generator,
        "--retrieve-limit",
        str(args.retrieve_limit),
        "--token-budget",
        str(args.token_budget),
        "--pins",
        args.pins,
        "--out",
        str(out_path),
        "--report-md",
        str(md_path),
    ]
    if args.max_probes is not None:
        argv += ["--max-probes", str(args.max_probes)]
    if args.max_instances is not None:
        argv += ["--max-instances", str(args.max_instances)]
    if args.seed is not None:
        argv += ["--seed", str(args.seed)]
    binary = _binary_for(name, args)
    if binary:
        argv += ["--binary", binary]
    data_dir = _data_dir_for(name, args)
    if data_dir:
        argv += ["--data-dir", data_dir]
    if args.cache_dir:
        argv += ["--cache-dir", args.cache_dir]
    return argv


def record_blocked_gate(args) -> int:
    """Record the blocked quotable-baseline gate and claim no number."""
    record = blocked_gate_record(
        system_names=list(SYSTEMS),
        judge=args.judge,
        generator=args.generator,
        detail=(
            "the two-system baseline needs a tier-2 judge and no frontier key "
            "is set; no probe was run, no score was produced, and tier 1 was "
            "not substituted"
        ),
    )
    try:
        record["systems"] = capture_fingerprints(
            list(SYSTEMS),
            binary_for=lambda name: _binary_for(name, args),
            data_dir_for=lambda name: _data_dir_for(name, args),
            retrieve_limit=args.retrieve_limit,
            pins=load_pins(Path(args.pins)),
        )
    except Exception as exc:  # noqa: BLE001 - the blocked record still gets written
        record["fingerprint_error"] = f"fingerprint capture could not start: {exc}"
    write_blocked_outputs(
        record,
        Path(args.out) if args.out else None,
        Path(args.report_md) if args.report_md else None,
    )
    print(
        "clio-bench baseline: blocked gate quotable-baseline: no frontier judge "
        "key is set; no baseline claimed",
        file=sys.stderr,
    )
    return 2


def merge_reports(reports: dict[str, dict]) -> dict:
    """Merge single-system runner reports into one baseline artifact.

    Each system keeps the runner's fingerprint fields at their own path and
    gains the scorecard it produced, so both systems are shaped identically.
    The top level names the systems that ran; there is no single top-level
    score, because one system's number is not the baseline's number.
    """
    systems: dict[str, dict] = {}
    for name, report in reports.items():
        block = dict(report["systems"][name])
        block["suites"] = report["suites"]
        block["overall"] = report["overall"]
        systems[name] = block
    first = reports[next(iter(reports))]
    run = dict(first["run"])
    run.pop("adapter", None)
    run["adapters"] = list(reports)
    return {
        "harness": first["harness"],
        "run": run,
        "pins": first["pins"],
        "systems": systems,
    }


def render_baseline_markdown(report: dict) -> str:
    """Render the two-system baseline with tier and probe count on every line."""
    run = report["run"]
    lines = [
        "# clio-bench baseline (two systems)",
        "",
        f"- Systems: {', '.join(run['adapters'])}",
        f"- Suite(s): {', '.join(run['suites'])}",
        f"- Partition: {run['partition']} (seed {run['seed']}, max probes {run['max_probes']})",
        f"- Judge: {run['judge']['model']} ({run['judge']['tier']})",
        f"- Generator: {run['generator']['model']}",
        f"- Tokenizer: {report['harness']['tokenizer']}",
        f"- Generated: {report['harness']['generated_at']}",
        "",
    ]
    for name, block in report["systems"].items():
        lines.append(f"## {name} (version {block['version']})")
        lines.append("")
        lines.extend(render_settings_lines(block["settings"]))
        lines.append("")
        for suite, suite_block in block["suites"].items():
            lines.extend(
                render_suite_section(
                    suite, suite_block, run["judge"]["tier"]
                )
            )
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if (args.judge == "tier2" or args.generator == "tier2") and not frontier_key_present():
        return record_blocked_gate(args)

    out_path = Path(args.out) if args.out else None
    md_path = Path(args.report_md) if args.report_md else None
    reports: dict[str, dict] = {}
    with tempfile.TemporaryDirectory(prefix="clio-bench-baseline-") as tmp:
        for name in SYSTEMS:
            runner_out = Path(tmp) / f"{name}.json"
            runner_md = Path(tmp) / f"{name}.md"
            code = cli.main(_runner_argv(name, args, runner_out, runner_md))
            if code != 0:
                print(
                    f"clio-bench baseline: {name} run exited {code}; no baseline "
                    "written, because a missing or broken system is a stop "
                    "condition and not a reason to run with one system",
                    file=sys.stderr,
                )
                return 2
            reports[name] = json.loads(runner_out.read_text(encoding="utf-8"))

    merged = merge_reports(reports)
    if out_path is not None:
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(json.dumps(merged, indent=2), encoding="utf-8")
    if md_path is not None:
        md_path.parent.mkdir(parents=True, exist_ok=True)
        md_path.write_text(render_baseline_markdown(merged), encoding="utf-8")

    for name, block in merged["systems"].items():
        aggregate = block["overall"]
        print(
            f"clio-bench baseline: {name} probes={aggregate['scored_probes']} "
            f"accuracy={aggregate['overall']['accuracy']:.4f} "
            f"tier={aggregate['tier']} quotable={aggregate['quotable']}"
        )
    return 0
