"""Unit tests for the canonical reference tokenizer."""

from __future__ import annotations

import unittest

from benchmarks.clio_bench.tokenizer import ReferenceTokenizer, get_tokenizer


class TokenizerTests(unittest.TestCase):
    def test_pins_cl100k_base(self) -> None:
        tokenizer = get_tokenizer("cl100k_base")
        self.assertIsInstance(tokenizer, ReferenceTokenizer)
        self.assertTrue(tokenizer.name.startswith("cl100k_base"))

    def test_rejects_other_encoding(self) -> None:
        with self.assertRaises(ValueError):
            get_tokenizer("p50k_base")

    def test_count_is_deterministic_and_positive(self) -> None:
        tokenizer = get_tokenizer()
        text = "Alice visited Paris in May 2024."
        first = tokenizer.count(text)
        second = tokenizer.count(text)
        self.assertEqual(first, second)
        self.assertGreater(first, 0)

    def test_empty_text_counts_zero(self) -> None:
        self.assertEqual(get_tokenizer().count(""), 0)

    def test_longer_text_counts_more(self) -> None:
        tokenizer = get_tokenizer()
        self.assertGreater(
            tokenizer.count("one two three four five six"),
            tokenizer.count("one"),
        )


if __name__ == "__main__":
    unittest.main()
