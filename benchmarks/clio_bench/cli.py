"""Command line for the clio-bench harness.

One job: parse the flags, build the selected adapter, judge, and generator from
the registry, drive every requested suite, and write the report. The suites
themselves are run by `runner.run_suite` and the scorecard is assembled by
`report`, so this module holds no measurement logic of its own.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from .doubles import StubGenerator, StubJudge
from .fetch import MissingSnapshotError, PinError, load_pins
from .fingerprint import FingerprintError
from .llm import frontier_key_present
from .report import (
    blocked_gate_record,
    build_report,
    write_blocked_outputs,
    write_outputs,
)
from .runner import (
    _DEFAULT_PINS,
    ADAPTER_BUILDERS,
    ADAPTER_NAMES,
    IMPLEMENTED_SUITES,
    AccountingError,
    capture_fingerprints,
    load_suite,
    run_suite,
)
from .stubs import STUB_SUITES, StubSuiteError, require_implemented
from .tokenizer import get_tokenizer
from .types import ProbeResult

def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="clio-bench", description="Run a memory benchmark suite.")
    parser.add_argument("--suite", default="both", choices=["both", *IMPLEMENTED_SUITES, *STUB_SUITES])
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
    suites = list(IMPLEMENTED_SUITES) if args.suite == "both" else [args.suite]
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
                tokenizer=tokenizer,
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

        if isinstance(exc, (ChatClientError, JudgeError, ClioAdapterError, AccountingError)):
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
    write_outputs(report, out_path, md_path)

    overall = report["overall"]["overall"]
    print(
        f"clio-bench: suites={','.join(suites)} partition={args.partition} "
        f"probes={report['overall']['scored_probes']} accuracy={overall['accuracy']:.4f} "
        f"token_f1={overall['token_f1']:.4f} tokenizer={tokenizer.name}"
    )
    return 0
