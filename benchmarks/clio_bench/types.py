"""Shared data model for the clio-bench harness.

Every suite adapter parses its pinned snapshot into the same shapes so the
runner, partition logic, judge, and score aggregation stay suite-agnostic.
"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass(frozen=True)
class DialogueTurn:
    """One ingested unit: a speaker's turn in a conversation session."""

    session_id: str
    turn_id: str
    speaker: str
    role: str
    text: str
    timestamp: str | None = None


@dataclass(frozen=True)
class Probe:
    """One evaluation question with its gold answer and category metadata."""

    probe_id: str
    suite: str
    instance_id: str
    question: str
    gold_answer: str
    category: str
    evidence_ids: tuple[str, ...] = ()
    expect_abstention: bool = False
    adversarial_answer: str | None = None


@dataclass
class Instance:
    """A session-isolated benchmark unit: its turns plus its probes.

    The partition step never splits an instance, so all turns and probes from
    one conversation (LoCoMo) or one haystack item (LongMemEval) stay together.
    """

    instance_id: str
    sessions: list[list[DialogueTurn]]
    probes: list[Probe]


@dataclass
class SuiteData:
    """A parsed suite snapshot plus the pin it was loaded from."""

    suite: str
    instances: list[Instance]
    pin: dict


# The kinds of write decline a system can report, in the harness's own
# vocabulary rather than any one system's error codes. A run counts them
# separately, because a duplicate's text is already stored while an admission
# refusal is not stored at all.
REFUSAL_ADMISSION = "admission"
REFUSAL_DUPLICATE = "duplicate"


@dataclass
class IngestResult:
    """Outcome of ingesting one turn into the system under test.

    `ok` is False only for a decline: the system chose not to store the turn,
    and the adapter records that outcome rather than raising. `refusal` names
    the kind of decline (`REFUSAL_ADMISSION` or `REFUSAL_DUPLICATE`); it is
    None when the turn was stored.

    A write that fails for any reason other than a decline is never reported
    here. The adapter raises instead, so the run stops rather than counting a
    configuration failure as a refusal by the system.
    """

    ok: bool
    latency_ms: float
    tokens_processed: int
    error: str | None = None
    refusal: str | None = None


@dataclass
class RetrievalResult:
    """Context returned for one probe under a hard token budget.

    `tokens_retrieved` is the cost of `context`, so the budget bounds the text
    the answer generator receives rather than a total of the system's own hit
    costs.
    """

    context: str
    latency_ms: float
    tokens_retrieved: int
    item_ids: tuple[str, ...] = ()


@dataclass
class JudgeScore:
    """Structured judge verdict for one candidate answer."""

    accuracy: float
    temporal_correctness: bool
    hallucination_penalty: float
    explanation: str
    raw: dict = field(default_factory=dict)


@dataclass
class ProbeResult:
    """Everything recorded for one scored probe."""

    probe_id: str
    suite: str
    instance_id: str
    category: str
    question: str
    gold_answer: str
    candidate_answer: str
    retrieved_context: str
    judge: JudgeScore | None
    token_f1: float
    retrieval_latency_ms: float
    generation_latency_ms: float
    tokens_retrieved: int
    error: str | None = None
