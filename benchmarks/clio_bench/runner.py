"""One-command runner for the clio-bench suites.

`load_suite` resolves and verifies a pinned snapshot; `run_suite` drives an
adapter, answer generator, and judge over a partition; `main` wires the CLI.
Failures fail loudly: a missing snapshot or an unavailable judge aborts the
run before any score report is written.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import sys
import time
from argparse import Namespace
from dataclasses import asdict
from datetime import datetime, timezone
from importlib import import_module
from pathlib import Path

from . import __version__
from .adapter import MemoryBenchmarkAdapter
from .doubles import StubGenerator, StubJudge
from .fetch import MissingSnapshotError, PinError, load_pins
from .fingerprint import Fingerprint, FingerprintError, unreadable_setting
from .llm import KEY_ENV_VARS, frontier_key_present
from .partition import partition_instances
from .scoring import aggregate, token_f1
from .stubs import STUB_SUITES, StubSuiteError, require_implemented
from .tokenizer import get_tokenizer
from .types import Instance, Probe, ProbeResult, SuiteData

_IMPLEMENTED = ("locomo", "longmemeval")
_DEFAULT_PINS = Path(__file__).resolve().parent / "pins.json"

# The executable each adapter drives when --binary is absent. One adapter owns
# one system, so a single flag cannot name the binary for whichever adapter was
# selected; each builder reads its own entry here rather than repeating the
# literal, and the contract test asserts the resolution against this map. An
# adapter that drives nothing in-process has no entry and no executable to
# default. Every key must name a registered adapter.
ADAPTER_DEFAULT_BINARIES = {
    "clio": "clio",
    "mnemosyne": "mnemosyne",
}

# The data directory each CLI-backed system reads when --data-dir is absent.
# The builders and the blocked-gate capture both resolve through this map, so
# a record written without a probe describes the same directory a scored run
# reads. Every key must name a registered adapter that takes a data directory.
ADAPTER_DEFAULT_DATA_DIRS = {
    "clio": "benchmarks/.cache/clio-data",
    "mnemosyne": "benchmarks/.cache/mnemosyne-data",
}


def default_data_dir(name: str, explicit: str | None) -> str | None:
    """Resolve one system's data directory, explicit flag over the map default.

    A system with no default (an in-process double) keeps whatever was passed,
    which is normally None because it reads nothing from disk.
    """
    default = ADAPTER_DEFAULT_DATA_DIRS.get(name)
    if default is None:
        return explicit
    return explicit or str(Path(default).resolve())


def _build_memory(tokenizer, options):
    """Build the deterministic in-process adapter."""
    from .adapters.memory import MemoryAdapter

    return MemoryAdapter(tokenizer)


def _build_clio(tokenizer, options):
    """Build the adapter that drives the real clio binary."""
    from .adapters.clio import ClioAdapter

    return ClioAdapter(
        binary=options.binary or ADAPTER_DEFAULT_BINARIES["clio"],
        data_dir=default_data_dir("clio", options.data_dir),
        tokenizer=tokenizer,
        limit=options.retrieve_limit,
    )


def _build_mnemosyne(tokenizer, options):
    """Build the adapter that drives the real mnemosyne binary."""
    from .adapters.mnemosyne import MnemosyneAdapter

    return MnemosyneAdapter(
        binary=options.binary or ADAPTER_DEFAULT_BINARIES["mnemosyne"],
        data_dir=default_data_dir("mnemosyne", options.data_dir),
        tokenizer=tokenizer,
        limit=options.retrieve_limit,
    )


# The registry. The CLI's `--adapter` choices and the adapter contract test
# both read it, so an adapter cannot be selectable without being held to the
# five-method contract, and adding one needs no edit outside this map.
ADAPTER_BUILDERS = {
    "memory": _build_memory,
    "clio": _build_clio,
    "mnemosyne": _build_mnemosyne,
}
ADAPTER_NAMES = tuple(ADAPTER_BUILDERS)


def _dataset_module(suite: str):
    return import_module(f".datasets.{suite}", package=__package__)


def load_suite(suite: str, pins: dict, cache_dir: Path) -> SuiteData:
    """Resolve, verify, and parse a suite snapshot."""
    require_implemented(suite)
    if suite not in _IMPLEMENTED:
        raise StubSuiteError(
            f"unknown suite {suite!r}; implemented suites: {list(_IMPLEMENTED)}"
        )
    pin = pins["suites"][suite]
    return _dataset_module(suite).load(pin, cache_dir)


def _rank(seed: int, probe_id: str) -> str:
    return hashlib.sha256(f"{seed}:{probe_id}".encode("utf-8")).hexdigest()


def select_work(
    instances: list[Instance], *, max_probes: int | None, seed: int
) -> list[tuple[Instance, list[Probe]]]:
    """Choose which probes to run, grouped by instance, preserving order.

    When `max_probes` caps the run, probes are sampled round-robin across
    categories so every category is represented and the choice is seeded and
    therefore reproducible.
    """
    pairs = [(inst, probe) for inst in instances for probe in inst.probes]
    if max_probes is not None and len(pairs) > max_probes:
        by_category: dict[str, list[tuple[Instance, Probe]]] = {}
        for inst, probe in pairs:
            by_category.setdefault(probe.category, []).append((inst, probe))
        for category in by_category:
            by_category[category].sort(key=lambda pair: _rank(seed, pair[1].probe_id))
        categories = sorted(by_category)
        selected: list[tuple[Instance, Probe]] = []
        index = 0
        while len(selected) < max_probes:
            added = False
            for category in categories:
                bucket = by_category[category]
                if index < len(bucket):
                    selected.append(bucket[index])
                    added = True
                    if len(selected) >= max_probes:
                        break
            if not added:
                break
            index += 1
        pairs = selected
    grouped: dict[str, tuple[Instance, list[Probe]]] = {}
    order: list[str] = []
    for inst, probe in pairs:
        if inst.instance_id not in grouped:
            grouped[inst.instance_id] = (inst, [])
            order.append(inst.instance_id)
        grouped[inst.instance_id][1].append(probe)
    return [grouped[key] for key in order]


def _failed_probe(suite: str, probe: Probe, error: str) -> ProbeResult:
    """A probe with no score because the adapter could not retrieve context."""
    return ProbeResult(
        probe_id=probe.probe_id,
        suite=suite,
        instance_id=probe.instance_id,
        category=probe.category,
        question=probe.question,
        gold_answer=probe.gold_answer,
        candidate_answer="",
        retrieved_context="",
        judge=None,
        token_f1=0.0,
        retrieval_latency_ms=0.0,
        generation_latency_ms=0.0,
        tokens_retrieved=0,
        error=error,
    )


def run_suite(
    suite: str,
    data: SuiteData,
    *,
    adapter: MemoryBenchmarkAdapter,
    generator,
    judge,
    partition: str,
    seed: int,
    fractions: tuple[float, float, float],
    max_probes: int | None,
    token_budget: int,
    progress=None,
    stats: dict | None = None,
    max_instances: int | None = None,
) -> list[ProbeResult]:
    """Drive one suite end to end and return per-probe results.

    When `stats` is provided it is filled with ingestion counters so the
    report can show how many turns the product accepted or rejected. A single
    turn or retrieval that the adapter cannot complete is recorded as a failed
    probe and the run continues; judge and generator failures still abort.
    """
    parts = partition_instances(data.instances, seed=seed, fractions=fractions)
    if partition not in parts:
        raise ValueError(f"unknown partition {partition!r}; expected train/canary/blind")
    instances = parts[partition]
    if max_instances is not None:
        instances = instances[:max_instances]
    work = select_work(instances, max_probes=max_probes, seed=seed)
    if stats is not None:
        stats.setdefault("instances", 0)
        stats.setdefault("turns_ingested", 0)
        stats.setdefault("turns_rejected", 0)
        stats["instances"] += len(work)
    results: list[ProbeResult] = []
    for instance, probes in work:
        adapter.reset(instance.instance_id)
        for session in instance.sessions:
            for turn in session:
                if stats is not None:
                    stats["turns_ingested"] += 1
                try:
                    ingested = adapter.ingest(instance.instance_id, turn)
                except Exception:  # noqa: BLE001 - one bad turn must not kill the run
                    if stats is not None:
                        stats["turns_rejected"] += 1
                    continue
                if stats is not None and not ingested.ok:
                    stats["turns_rejected"] += 1
        for probe in probes:
            try:
                retrieval = adapter.retrieve(
                    instance.instance_id, probe.question, token_budget
                )
            except Exception as exc:  # noqa: BLE001 - record the miss, never fabricate
                results.append(_failed_probe(suite, probe, f"retrieval failed: {exc}"))
                continue
            started = time.monotonic()
            candidate = generator.answer(probe.question, retrieval.context)
            generation_ms = (time.monotonic() - started) * 1000.0
            score = judge.score(probe, candidate)
            results.append(
                ProbeResult(
                    probe_id=probe.probe_id,
                    suite=suite,
                    instance_id=probe.instance_id,
                    category=probe.category,
                    question=probe.question,
                    gold_answer=probe.gold_answer,
                    candidate_answer=candidate,
                    retrieved_context=retrieval.context,
                    judge=score,
                    token_f1=token_f1(candidate, probe.gold_answer),
                    retrieval_latency_ms=retrieval.latency_ms,
                    generation_latency_ms=generation_ms,
                    tokens_retrieved=retrieval.tokens_retrieved,
                )
            )
            if progress is not None:
                progress(suite, probe.probe_id, score.accuracy)
    return results


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


def _expected_settings(name: str) -> list[str]:
    """The settings a system's own surfaces expose, for a degraded record."""
    try:
        module = import_module(f".adapters.{name}", package=__package__)
    except ImportError:
        return []
    return list(getattr(module, "REQUIRED_SETTINGS", ()))


def capture_fingerprints(
    systems: list[str],
    *,
    binary_for,
    data_dir_for,
    retrieve_limit: int,
    pins: dict,
) -> dict[str, dict]:
    """Read each named system's fingerprint without running a probe.

    Used only for a blocked-gate record. Each system is built through the
    registry the way a scored run builds it, with the same default data
    directory, so the record describes the configuration the run would have
    used. Every block carries `fingerprint_complete`; a read that fails is an
    explicit incomplete block naming the system's expected settings, never a
    clean-looking stub.
    """
    tokenizer = get_tokenizer(pins["judge"]["tokenizer"])
    captured: dict[str, dict] = {}
    for name in systems:
        try:
            options = Namespace(
                binary=binary_for(name),
                data_dir=default_data_dir(name, data_dir_for(name)),
                retrieve_limit=retrieve_limit,
            )
            adapter = ADAPTER_BUILDERS[name](tokenizer, options)
            fingerprint = adapter.fingerprint()
            fingerprint.validate()
            block = fingerprint.to_json()
            block["fingerprint_complete"] = True
        except Exception as exc:  # noqa: BLE001 - the miss is recorded, never hidden
            block = {
                "system": name,
                "version": "unreadable",
                "fingerprint_complete": False,
                "expected_settings": _expected_settings(name),
                "settings": {
                    "fingerprint": unreadable_setting(
                        tried=(f"fingerprint read failed: {exc}",)
                    ).to_json()
                },
            }
        captured[name] = block
    return captured


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
        lines.append(
            f"Ingestion: {ingestion.get('instances', 0)} instances, "
            f"{ingestion.get('turns_ingested', 0)} turns "
            f"({ingestion.get('turns_rejected', 0)} rejected by admission)"
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


def _write_outputs(report: dict, out_path: Path | None, md_path: Path | None) -> None:
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


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="clio-bench", description="Run a memory benchmark suite.")
    parser.add_argument("--suite", default="both", choices=["both", *_IMPLEMENTED, *STUB_SUITES])
    parser.add_argument("--partition", default="canary", choices=["train", "canary", "blind"])
    parser.add_argument("--adapter", default="memory", choices=list(ADAPTER_NAMES))
    parser.add_argument("--judge", default="stub", choices=["stub", "local", "tier2"])
    parser.add_argument("--generator", default="stub", choices=["stub", "local", "tier2"])
    parser.add_argument("--max-probes", type=int, default=None)
    parser.add_argument("--max-instances", type=int, default=None)
    parser.add_argument("--token-budget", type=int, default=4096)
    parser.add_argument("--generator-max-tokens", type=int, default=96)
    parser.add_argument("--judge-max-tokens", type=int, default=200)
    parser.add_argument("--seed", type=int, default=None)
    parser.add_argument(
        "--binary",
        default=None,
        help=(
            "Executable the selected adapter drives. Defaults to the "
            "adapter's own system: clio, or mnemosyne."
        ),
    )
    parser.add_argument("--data-dir", default=None)
    parser.add_argument("--retrieve-limit", type=int, default=10)
    parser.add_argument("--cache-dir", default=None)
    parser.add_argument("--judge-base-url", default=None)
    parser.add_argument("--judge-model", default=None)
    parser.add_argument("--generator-base-url", default=None)
    parser.add_argument("--generator-model", default=None)
    parser.add_argument("--out", default=None)
    parser.add_argument("--report-md", default=None)
    parser.add_argument("--pins", default=str(_DEFAULT_PINS))
    parser.add_argument("--list-stubs", action="store_true")
    parser.add_argument("--verbose", action="store_true")
    return parser


def _build_adapter(args, tokenizer):
    """Build the registered adapter named by `args.adapter`."""
    try:
        builder = ADAPTER_BUILDERS[args.adapter]
    except KeyError:
        raise ValueError(
            f"unknown adapter {args.adapter!r}; registered: {list(ADAPTER_NAMES)}"
        ) from None
    return builder(tokenizer, args)


def _build_judge(args, pins):
    if args.judge == "stub":
        return StubJudge(), {"tier": "stub", "model": "stub-exact-match", "base_url": ""}
    tier = "tier1" if args.judge == "local" else "tier2"
    settings = pins["judge"][tier]
    model = args.judge_model or settings["model"]
    base_url = args.judge_base_url or settings["base_url"]
    from .judges.llm_judge import LlmJudge
    from .llm import HttpChatClient

    return LlmJudge(
        HttpChatClient(base_url, model),
        model,
        tier=tier,
        max_tokens=args.judge_max_tokens,
    ), {
        "tier": tier,
        "model": model,
        "base_url": base_url,
    }


def _build_generator(args, pins):
    if args.generator == "stub":
        return StubGenerator(), {"model": "stub-context-echo", "base_url": ""}
    tier = "tier1" if args.generator == "local" else "tier2"
    settings = pins["judge"][tier]
    model = args.generator_model or settings["model"]
    base_url = args.generator_base_url or settings["base_url"]
    from .answerer import AnswerGenerator
    from .llm import HttpChatClient

    return AnswerGenerator(
        HttpChatClient(base_url, model),
        model,
        max_tokens=args.generator_max_tokens,
    ), {
        "model": model,
        "base_url": base_url,
    }


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)

    if args.list_stubs:
        for key, stub in STUB_SUITES.items():
            print(f"{key}: {stub['name']} - NOT IMPLEMENTED - {stub['reason']}")
        return 0

    if (args.judge == "tier2" or args.generator == "tier2") and not frontier_key_present():
        record = blocked_gate_record(
            system_names=[args.adapter],
            judge=args.judge,
            generator=args.generator,
            detail=(
                "a tier-2 judge or generator was requested and no frontier key "
                "is set; no probe was run and no score was produced"
            ),
        )
        try:
            record["systems"] = capture_fingerprints(
                [args.adapter],
                binary_for=lambda name: args.binary,
                data_dir_for=lambda name: args.data_dir,
                retrieve_limit=args.retrieve_limit,
                pins=load_pins(Path(args.pins)),
            )
        except Exception as exc:  # noqa: BLE001 - the blocked record is still written
            record["fingerprint_error"] = (
                f"fingerprint capture could not start: {exc}"
            )
        write_blocked_outputs(
            record,
            Path(args.out) if args.out else None,
            Path(args.report_md) if args.report_md else None,
        )
        print(
            "clio-bench: blocked gate quotable-baseline: no frontier judge key "
            "is set; no probe run, no score produced, tier 1 not substituted",
            file=sys.stderr,
        )
        return 2

    pins = load_pins(Path(args.pins))
    suites = list(_IMPLEMENTED) if args.suite == "both" else [args.suite]
    try:
        for suite in suites:
            require_implemented(suite)
    except StubSuiteError as exc:
        print(f"clio-bench: {exc}", file=sys.stderr)
        return 2

    seed = args.seed if args.seed is not None else pins["partition"]["seed"]
    fractions = (
        pins["partition"]["train"],
        pins["partition"]["canary"],
        pins["partition"]["blind"],
    )
    tokenizer = get_tokenizer(pins["judge"]["tokenizer"])
    cache_dir = Path(args.cache_dir or "benchmarks/.cache/datasets")
    adapter = _build_adapter(args, tokenizer)
    judge, judge_meta = _build_judge(args, pins)
    generator, generator_meta = _build_generator(args, pins)

    if not adapter.healthcheck():
        print(f"clio-bench: adapter {args.adapter!r} is not ready", file=sys.stderr)
        return 2

    try:
        fingerprint = adapter.fingerprint()
        fingerprint.validate()
    except FingerprintError as exc:
        print(
            f"clio-bench: configuration fingerprint failed, no score produced: {exc}",
            file=sys.stderr,
        )
        return 2
    judge_meta["quotable"] = judge_meta["tier"] == "tier2"

    suite_results: dict[str, list[ProbeResult]] = {}
    suite_stats: dict[str, dict] = {}

    def progress(suite: str, probe_id: str, accuracy: float) -> None:
        print(f"[{suite}] {probe_id} accuracy={accuracy:.3f}", file=sys.stderr)

    try:
        for suite in suites:
            if args.verbose:
                print(f"clio-bench: loading suite {suite}", file=sys.stderr)
            data = load_suite(suite, pins, cache_dir)
            stats: dict = {}
            suite_results[suite] = run_suite(
                suite,
                data,
                adapter=adapter,
                generator=generator,
                judge=judge,
                partition=args.partition,
                seed=seed,
                fractions=fractions,
                max_probes=args.max_probes,
                token_budget=args.token_budget,
                progress=progress if args.verbose else None,
                stats=stats,
                max_instances=args.max_instances,
            )
            suite_stats[suite] = stats
    except (PinError, MissingSnapshotError) as exc:
        print(f"clio-bench: snapshot failure, no score produced: {exc}", file=sys.stderr)
        return 2
    except Exception as exc:  # noqa: BLE001 - judge/endpoint failures abort the run
        from .adapters.clio import ClioAdapterError
        from .judges.llm_judge import JudgeError
        from .llm import ChatClientError

        if isinstance(exc, (ChatClientError, JudgeError, ClioAdapterError)):
            print(
                f"clio-bench: judge/generator/adapter failure, no score produced: {exc}",
                file=sys.stderr,
            )
            return 2
        raise

    run_meta = {
        "suites": suites,
        "partition": args.partition,
        "seed": seed,
        "max_probes": args.max_probes,
        "max_instances": args.max_instances,
        "token_budget": args.token_budget,
        "adapter": args.adapter,
        "judge": judge_meta,
        "generator": generator_meta,
    }
    report = build_report(
        pins=pins,
        run_meta=run_meta,
        suite_results=suite_results,
        tokenizer_name=tokenizer.name,
        fingerprint=fingerprint,
        suite_stats=suite_stats,
    )
    out_path = Path(args.out) if args.out else None
    md_path = Path(args.report_md) if args.report_md else None
    _write_outputs(report, out_path, md_path)

    overall = report["overall"]["overall"]
    print(
        f"clio-bench: suites={','.join(suites)} partition={args.partition} "
        f"probes={report['overall']['scored_probes']} accuracy={overall['accuracy']:.4f} "
        f"token_f1={overall['token_f1']:.4f} tokenizer={tokenizer.name}"
    )
    return 0
