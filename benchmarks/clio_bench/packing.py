"""Budget-aware packing of retrieval hits into one context string.

Every adapter returns its hits through this module, so the charge for a hit is
the cost of the text the run hands over rather than the cost of a shorter
substring of it. Charging a hit's own text and then composing a prefix and a
newline separator that no charge included is what made the reported
`tokens_retrieved` smaller than the context and let a pack pass its budget.
"""

from __future__ import annotations

from .tokenizer import ReferenceTokenizer


def pack_context(
    hits: list[tuple[str, str]],
    token_budget: int,
    tokenizer: ReferenceTokenizer,
) -> tuple[str, int, tuple[str, ...]]:
    """Pack `(item_id, line)` hits into one newline-joined context string.

    The cost of a candidate pack is measured on the composed string itself,
    because a byte-pair encoding is not a sum over its parts: the same text
    costs a different number of tokens depending on what sits either side of
    it, so a total of per-hit charges cannot bound the string the caller
    receives. The returned cost is the cost of the returned context and is
    never above `token_budget`.

    A hit that does not fit stops the pack rather than being skipped, so the
    composed context ranks the way the system returned it.

    One candidate is measured per hit offered, and the loop stops at the first
    hit that does not fit, so the work grows with the pack it accepted rather
    than with the hits offered. At the harness limits (`--retrieve-limit` 10,
    `--token-budget` 4096) that is unmeasurable; the repeated measurement shows
    only if a very large limit is combined with a very large budget.
    """
    pieces: list[str] = []
    item_ids: list[str] = []
    used = 0
    for item_id, line in hits:
        candidate = "\n".join([*pieces, line])
        cost = tokenizer.count(candidate)
        if cost > token_budget:
            break
        pieces.append(line)
        item_ids.append(item_id)
        used = cost
    return "\n".join(pieces), used, tuple(item_ids)


def accounting_mismatch(
    context: str, tokens_retrieved: int, tokenizer: ReferenceTokenizer
) -> str | None:
    """Return a message when a reported cost is not the cost of its context.

    One implementation of the invariant the interface states, so a run can
    refuse a report and a test can fail on the same comparison in the same
    words. An adapter that packs through `pack_context` cannot reach a
    mismatch; one that builds its own context can.
    """
    measured = tokenizer.count(context)
    if measured == tokens_retrieved:
        return None
    return f"tokens_retrieved={tokens_retrieved} but the context costs {measured}"

