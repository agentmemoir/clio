"""Unit tests for the documented stub suites."""

from __future__ import annotations

import unittest

from benchmarks.clio_bench.stubs import STUB_SUITES, StubSuiteError, require_implemented


class StubTests(unittest.TestCase):
    def test_three_stub_suites_recorded(self) -> None:
        self.assertEqual(set(STUB_SUITES), {"beam", "memoryagentbench", "ama-bench"})

    def test_each_stub_is_explicitly_not_implemented(self) -> None:
        for stub in STUB_SUITES.values():
            self.assertIs(stub["implemented"], False)
            self.assertTrue(stub["reason"])
            self.assertTrue(stub["owner"])

    def test_require_implemented_rejects_stubs(self) -> None:
        for suite in STUB_SUITES:
            with self.assertRaises(StubSuiteError):
                require_implemented(suite)

    def test_require_implemented_allows_real_suites(self) -> None:
        require_implemented("locomo")
        require_implemented("longmemeval")


if __name__ == "__main__":
    unittest.main()
