"""Unit tests for budget-aware packing of retrieval hits.

The packing rule is asserted against the harness's own tokenizer, so these
tests state no token number of their own: what is checked is that a hit is
charged at the width it is composed with, and that the charge reported is the
cost of the string the caller receives.
"""

from __future__ import annotations

import unittest

from benchmarks.clio_bench.packing import pack_context
from benchmarks.clio_bench.tokenizer import get_tokenizer

# Lines shaped like the ones the adapters compose: an id prefix, then the text
# the system returned, which is the part the old charge covered on its own.
_FIRST = "[t1] user: alpha beta gamma"
_SECOND = "[t2] user: delta epsilon"
_LONGER = "[t3] user: a much longer piece of retrieved context text here"


class PackingTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tokenizer = get_tokenizer()

    def test_nothing_to_pack_returns_an_empty_context(self) -> None:
        self.assertEqual(pack_context([], 4096, self.tokenizer), ("", 0, ()))

    def test_a_budget_of_zero_packs_nothing(self) -> None:
        # Every line costs at least one token, so no pack is inside a zero
        # budget: the call returns an empty context rather than one line.
        self.assertEqual(
            pack_context([("t1", _FIRST)], 0, self.tokenizer), ("", 0, ())
        )

    def test_the_cost_reported_is_the_cost_of_the_context_returned(self) -> None:
        context, used, item_ids = pack_context(
            [("t1", _FIRST), ("t2", _SECOND)], 4096, self.tokenizer
        )
        self.assertEqual(context, f"{_FIRST}\n{_SECOND}")
        self.assertEqual(item_ids, ("t1", "t2"))
        self.assertEqual(used, self.tokenizer.count(context))

    def test_the_cost_reported_is_a_bound_on_the_context_returned(self) -> None:
        # One token short of the two-line pack: the second line must not be
        # admitted, because the context handed over would be over budget.
        full = self.tokenizer.count(f"{_FIRST}\n{_SECOND}")
        self.assertGreater(full, self.tokenizer.count(_FIRST))
        context, used, item_ids = pack_context(
            [("t1", _FIRST), ("t2", _SECOND)], full - 1, self.tokenizer
        )
        self.assertEqual(context, _FIRST)
        self.assertEqual(item_ids, ("t1",))
        self.assertEqual(used, self.tokenizer.count(_FIRST))
        self.assertLessEqual(self.tokenizer.count(context), full - 1)
        self.assertLessEqual(used, full - 1)

    def test_a_line_is_charged_with_the_prefix_composed_around_it(self) -> None:
        # A budget that covers the bare text must not admit the decorated line.
        # An adapter that charged the text alone would pack this line and hand
        # over more than the budget allowed.
        bare_cost = self.tokenizer.count("alpha beta gamma")
        self.assertGreater(self.tokenizer.count(_FIRST), bare_cost)
        self.assertEqual(
            pack_context([("t1", _FIRST)], bare_cost, self.tokenizer), ("", 0, ())
        )

    def test_a_hit_that_does_not_fit_stops_the_pack_instead_of_being_skipped(
        self,
    ) -> None:
        # The middle hit is longer than the hit that would fit after the first,
        # and it must not be skipped: a later, smaller hit taking its budget
        # would rank the composed context below what the system returned.
        self.assertGreater(
            self.tokenizer.count(_LONGER), self.tokenizer.count(_SECOND)
        )
        budget = self.tokenizer.count(f"{_FIRST}\n{_SECOND}")
        context, used, item_ids = pack_context(
            [("t1", _FIRST), ("t3", _LONGER), ("t2", _SECOND)],
            budget,
            self.tokenizer,
        )
        self.assertEqual(context, _FIRST)
        self.assertEqual(item_ids, ("t1",))
        self.assertEqual(used, self.tokenizer.count(_FIRST))


if __name__ == "__main__":
    unittest.main()
