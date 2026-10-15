"""Running one clio-bench suite over an adapter, answer generator, and judge.

`run_suite` drives the trio over the selected probes and returns per-probe results. Failures fail loudly: an adapter that reports a retrieval cost which is not the cost of its context raises `AccountingError` rather than putting that number into a report, and a suite whose writes all failed raises `EmptyStoreError` instead of scoring a bank that holds nothing.
"""

from __future__ import annotations

import time

from .adapter import AdapterError, MemoryBenchmarkAdapter
from .packing import accounting_mismatch
from .partition import partition_instances
from .scoring import token_f1
from .tokenizer import ReferenceTokenizer
from .types import (
    REFUSAL_ADMISSION,
    REFUSAL_DUPLICATE,
    REFUSAL_FILTERED,
    Probe,
    ProbeResult,
    SuiteData,
)
from .work_selection import select_work

# Ingestion counters, one per kind of decline an adapter can name. Every row is
# rendered in the scorecard, so a reader never has to work out what a count
# means, and a decline kind with no row stops the run instead of being absorbed
# as an admission refusal.
INGEST_COUNTERS = {
    REFUSAL_ADMISSION: "turns_rejected",
    REFUSAL_DUPLICATE: "turns_duplicate",
    REFUSAL_FILTERED: "turns_filtered",
}


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


def _decline_counter(refusal: str | None) -> str:
    """Return the counter one kind of decline is counted in.

    The vocabulary is the harness's own, so a decline kind this module does not
    know is an adapter bug, not a decision: counting it as an admission refusal
    is the mislabelling issue #21 is about, so the run stops instead.
    """
    try:
        return INGEST_COUNTERS[refusal]
    except KeyError:
        raise AdapterError(
            f"a declined write must name one of {sorted(INGEST_COUNTERS)}; "
            f"got {refusal!r}"
        ) from None


def _unscoreable_bank(
    suite: str,
    probes: list[Probe],
    *,
    stored: int,
    declined: int,
    failed: list[str],
) -> list[ProbeResult]:
    """Record an instance's probes as failed when its bank cannot be measured.

    Two cases, and neither may be scored. A write that failed leaves the store
    missing a turn, so the retrieval numbers would be wrong for a reason the
    report cannot correct. A bank the system declined to fill holds nothing at
    all. Both record the probes as failed and name the cause, because a score
    here would read as a bank merely filtered by admission.
    """
    if failed:
        reason = (
            f"the harness could not write {len(failed)} of this instance's "
            f"turns ({stored} stored, {declined} declined), so its store is "
            f"incomplete. First failure: {failed[0]}"
        )
    elif declined:
        reason = (
            f"the system stored none of this instance's turns "
            f"({declined} declined), so there was nothing to retrieve from"
        )
    else:
        reason = (
            "this instance carries no turns, so nothing was stored and there "
            "was nothing to retrieve from"
        )
    return [_failed_probe(suite, probe, reason) for probe in probes]


class AccountingError(RuntimeError):
    """Raised when an adapter's reported cost is not the cost of its context.

    Going on would write a report whose every probe carries a number that is not
    the cost of the context handed to the answer generator, so the run stops and
    no report is written, the way an unavailable judge stops it.
    """


class EmptyStoreError(RuntimeError):
    """Raised when a suite stored nothing because its writes failed.

    A score from a bank that holds nothing is not a measurement, so the run
    stops and no report is written, the way an unavailable judge stops it. A
    store that took nothing because the system declined every turn is the
    system's own decision and is handled apart: its probes are recorded as
    failed rather than scored, and the run continues.
    """


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
    tokenizer: ReferenceTokenizer,
    progress=None,
    stats: dict | None = None,
    max_instances: int | None = None,
) -> list[ProbeResult]:
    """Drive one suite end to end and return per-probe results.

    When `stats` is provided it is filled with one counter per ingestion
    outcome, so the report can say how many turns the system stored, how many it
    declined and in what way, and how many the harness failed to write. A
    decline is the system's own decision and is counted per kind; a write that
    fails is recorded and the run continues, the way a failed retrieval is.

    Nothing is ever scored against a bank that cannot answer for the instance.
    An instance whose store is missing a failed write, or that stored none of
    its turns, has its probes recorded as failed with the cause named instead.
    A suite that stored nothing at all while writes were failing stops the run
    with `EmptyStoreError`, because that is a broken write path rather than a
    selective system. A retrieval the adapter cannot complete is recorded as a
    failed probe too; judge and generator failures still abort, and so does an
    adapter that reports a retrieval cost which is not the cost of the context
    it returned, because that number goes into every probe of the report.
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
        stats.setdefault("turns_stored", 0)
        stats.setdefault("turns_failed", 0)
        for counter in INGEST_COUNTERS.values():
            stats.setdefault(counter, 0)
        stats["instances"] += len(work)
    results: list[ProbeResult] = []
    suite_stored = 0
    suite_failed: list[str] = []
    for instance, probes in work:
        adapter.reset(instance.instance_id)
        stored = 0
        declined = 0
        failed: list[str] = []
        for session in instance.sessions:
            for turn in session:
                if stats is not None:
                    stats["turns_ingested"] += 1
                try:
                    ingested = adapter.ingest(instance.instance_id, turn)
                except AdapterError as exc:
                    # One turn the harness could not write is recorded and the
                    # run continues, the way a failed retrieval is; the instance
                    # is left unscoreable below, because its store is now
                    # missing a turn the probes would be answered from.
                    failed.append(str(exc))
                    suite_failed.append(str(exc))
                    if stats is not None:
                        stats["turns_failed"] += 1
                    continue
                if ingested.ok:
                    stored += 1
                    if stats is not None:
                        stats["turns_stored"] += 1
                    continue
                counter = _decline_counter(ingested.refusal)
                declined += 1
                if stats is not None:
                    stats[counter] += 1
        suite_stored += stored
        if failed or stored == 0:
            results.extend(
                _unscoreable_bank(
                    suite,
                    probes,
                    stored=stored,
                    declined=declined,
                    failed=failed,
                )
            )
            continue
        for probe in probes:
            try:
                retrieval = adapter.retrieve(
                    instance.instance_id, probe.question, token_budget
                )
            except Exception as exc:  # noqa: BLE001 - record the miss, never fabricate
                results.append(_failed_probe(suite, probe, f"retrieval failed: {exc}"))
                continue
            mismatch = accounting_mismatch(
                retrieval.context, retrieval.tokens_retrieved, tokenizer
            )
            if mismatch is not None:
                raise AccountingError(
                    f"{type(adapter).__name__} reported the wrong retrieval cost "
                    f"for {probe.probe_id} in suite {suite!r}: {mismatch}"
                )
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
    if suite_failed and suite_stored == 0:
        raise EmptyStoreError(
            f"suite {suite!r}: {len(suite_failed)} writes failed and no turn was "
            f"stored, so every score would measure an empty bank. "
            f"First failure: {suite_failed[0]}"
        )
    return results
