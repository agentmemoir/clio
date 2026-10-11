"""Report assembly for the clio-bench harness.

One job: turn per-probe results into the JSON report and its Markdown scorecard.
Nothing here drives a system or decides what to run; `runner.py` produces the
results and `cli.py` wires the command line. The blocked-gate record is a report
shape too, so it lives here with the scorecard it replaces.
"""

from __future__ import annotations

import json
import os
import platform
from dataclasses import asdict
from datetime import datetime, timezone
from pathlib import Path

from . import __version__
from .fingerprint import Fingerprint
from .llm import KEY_ENV_VARS
from .scoring import aggregate
from .types import ProbeResult

def _labelled(block: dict, tier: str, quotable: bool) -> dict:
    """Attach tier and quotability to one aggregate block, additively.

    Only tier 2 is quotable as a result, so the report itself carries the
    verdict rather than leaving a reader to infer it from a model name.
    """
    block["tier"] = tier
    block["quotable"] = quotable
    return block


def build_report(
    *,
    pins: dict,
    run_meta: dict,
    suite_results: dict[str, list[ProbeResult]],
    tokenizer_name: str,
    fingerprint: Fingerprint,
    suite_stats: dict[str, dict] | None = None,
) -> dict:
    """Assemble the JSON report with per-suite and overall aggregates.

    The fingerprint is required: a report whose settings are unknown is the
    anecdote the fingerprint exists to prevent, so there is no code path that
    builds one without it.
    """
    fingerprint.validate()
    all_results = [r for results in suite_results.values() for r in results]
    stats = suite_stats or {}
    tier = run_meta["judge"]["tier"]
    quotable = tier == "tier2"
    return {
        "harness": {
            "name": "clio-bench",
            "version": __version__,
            "python": platform.python_version(),
            "tokenizer": tokenizer_name,
            "generated_at": datetime.now(timezone.utc).isoformat(),
        },
        "run": run_meta,
        "pins": {
            suite: {k: pin.get(k) for k in ("name", "commit", "sha256", "git_blob")}
            for suite, pin in pins["suites"].items()
        },
        "systems": {fingerprint.system: fingerprint.to_json()},
        "overall": _labelled(aggregate(all_results), tier, quotable),
        "suites": {
            suite: {
                "aggregate": _labelled(aggregate(results), tier, quotable),
                "ingestion": stats.get(suite, {}),
                "probes": [asdict(result) for result in results],
            }
            for suite, results in suite_results.items()
        },
    }


_VALUE_PREVIEW_LIMIT = 120


def _bounded_value(value: object) -> str:
    """Return a one-line, bounded preview of a captured setting value."""
    text = json.dumps(value, sort_keys=True, default=str)
    text = " ".join(text.split())
    if len(text) > _VALUE_PREVIEW_LIMIT:
        return text[: _VALUE_PREVIEW_LIMIT - 1] + "…"
    return text


def render_settings_lines(settings: dict) -> list[str]:
    """Render one system's setting records as Markdown bullets.

    A readable field carries the captured value, bounded to one line, and
    names the surface that returned it. An unreadable field names what was
    tried, so an absent surface is visible instead of blank.
    """
    lines: list[str] = []
    for field, record in settings.items():
        if record["status"] == "read":
            lines.append(
                f"- `{field}`: `{_bounded_value(record['value'])}` "
                f"(read through `{record['surface']}`)"
            )
        else:
            tried = "; ".join(record["tried"])
            lines.append(f"- `{field}`: not readable; tried {tried}")
    return lines


def render_suite_section(suite: str, block: dict, tier: str) -> list[str]:
    """Render one suite's numbers, each label naming tier and probe count."""
    aggregate_block = block["aggregate"]
    label = f"[tier {tier}, {aggregate_block['scored_probes']} probes"
    if tier != "tier2":
        label += "; not quotable"
    label += "]"
    lines = [f"## {suite}", ""]
    lines.append(
        f"Scored probes: {aggregate_block['scored_probes']} "
        f"(failed {aggregate_block['failed_probes']}) {label}; "
        f"overall accuracy {aggregate_block['overall']['accuracy']:.4f}; "
        f"token F1 {aggregate_block['overall']['token_f1']:.4f}"
    )
    ingestion = block.get("ingestion") or {}
    if ingestion:
        declined = f"{ingestion.get('turns_rejected', 0)} rejected by admission"
        duplicates = ingestion.get("turns_duplicate", 0)
        if duplicates:
            declined += f", {duplicates} already stored"
        lines.append(
            f"Ingestion: {ingestion.get('instances', 0)} instances, "
            f"{ingestion.get('turns_ingested', 0)} turns ({declined})"
        )
    lines.append("")
    lines.append(f"| Category {label} | Count | Accuracy | Token F1 | p95 retrieval ms |")
    lines.append("|---|---|---|---|---|")
    for category, metrics in aggregate_block["per_category"].items():
        lines.append(
            f"| {category} | {metrics['count']} | {metrics['accuracy']:.4f} | "
            f"{metrics['token_f1']:.4f} | {metrics['retrieval_latency_ms_p95']:.1f} |"
        )
    lines.append("")
    return lines


def render_markdown(report: dict) -> str:
    """Render a compact scorecard from a report dictionary."""
    run = report["run"]
    lines = [
        "# clio-bench scorecard",
        "",
        f"- Suite(s): {', '.join(run['suites'])}",
        f"- Partition: {run['partition']} (seed {run['seed']}, max probes {run['max_probes']})",
        f"- Adapter: {run['adapter']}",
        f"- Judge: {run['judge']['model']} ({run['judge']['tier']})",
        f"- Generator: {run['generator']['model']}",
        f"- Tokenizer: {report['harness']['tokenizer']}",
        f"- Generated: {report['harness']['generated_at']}",
        "",
        "## Configuration",
        "",
    ]
    for name, block in report["systems"].items():
        lines.append(f"### {name} {block['version']}")
        lines.append("")
        lines.extend(render_settings_lines(block["settings"]))
        lines.append("")
    for suite, block in report["suites"].items():
        lines.extend(render_suite_section(suite, block, run["judge"]["tier"]))
    return "\n".join(lines) + "\n"


def write_outputs(report: dict, out_path: Path | None, md_path: Path | None) -> None:
    if out_path is not None:
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(json.dumps(report, indent=2), encoding="utf-8")
    if md_path is not None:
        md_path.parent.mkdir(parents=True, exist_ok=True)
        md_path.write_text(render_markdown(report), encoding="utf-8")


def blocked_gate_record(
    *, system_names: list[str], judge: str, generator: str, detail: str
) -> dict:
    """The record written when the quotable-baseline gate cannot run.

    No probe has run and no score exists. The record exists so the blocked
    outcome is stated in a file rather than inferred from a missing one, and
    so a reader can see the reason without asking anyone. Only whether a key
    variable is set is recorded, never a value.
    """
    return {
        "harness": {
            "name": "clio-bench",
            "version": __version__,
            "python": platform.python_version(),
            "generated_at": datetime.now(timezone.utc).isoformat(),
        },
        "status": "blocked",
        "blocked_gate": {
            "gate": "quotable-baseline",
            "reason": "frontier judge key absent",
            "systems": list(system_names),
            "requested": {"judge": judge, "generator": generator},
            "requires_env": list(KEY_ENV_VARS),
            "present_env": {
                name: bool(os.environ.get(name)) for name in KEY_ENV_VARS
            },
            "detail": detail,
        },
    }


def render_blocked_markdown(record: dict) -> str:
    """Render a blocked-gate record as Markdown, configuration included.

    The systems block is rendered when the record carries one, so the
    configuration the run would have used is readable without opening the
    JSON. A record whose fingerprint capture could not start says so.
    """
    gate = record["blocked_gate"]
    present = ", ".join(
        f"{name}={value}" for name, value in gate["present_env"].items()
    )
    lines = [
        "# clio-bench blocked gate",
        "",
        f"- Gate: {gate['gate']}",
        f"- Reason: {gate['reason']}",
        f"- Systems: {', '.join(gate['systems'])}",
        f"- Judge requested: {gate['requested']['judge']}",
        f"- Generator requested: {gate['requested']['generator']}",
        f"- Key variables required: {', '.join(gate['requires_env'])}",
        f"- Variables set: {present}",
        f"- Detail: {gate['detail']}",
        "",
        "No probe was run, no score was produced, and tier 1 was not substituted.",
    ]
    if "systems" in record:
        lines += ["", "## Configuration", ""]
        for name, block in record["systems"].items():
            lines.append(f"### {name} {block.get('version', '')}")
            lines.append("")
            if not block.get("fingerprint_complete", False):
                expected = ", ".join(block.get("expected_settings", ()))
                lines.append(
                    f"- Fingerprint incomplete; the system's own surfaces "
                    f"expose: {expected or 'no named minimum'}"
                )
            lines.extend(render_settings_lines(block["settings"]))
            lines.append("")
    if "fingerprint_error" in record:
        lines += [
            f"- Fingerprint capture error: {record['fingerprint_error']}",
            "",
        ]
    return "\n".join(lines) + "\n"


def write_blocked_outputs(
    record: dict, out_path: Path | None, md_path: Path | None
) -> None:
    """Write a blocked-gate record to the JSON and Markdown paths when given."""
    if out_path is not None:
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(json.dumps(record, indent=2), encoding="utf-8")
    if md_path is not None:
        md_path.parent.mkdir(parents=True, exist_ok=True)
        md_path.write_text(render_blocked_markdown(record), encoding="utf-8")


