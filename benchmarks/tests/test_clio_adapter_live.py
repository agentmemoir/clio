"""Live contract checks for the Clio adapter's write classification.

The hermetic suite drives a fake CLI, which can only confirm that the harness
agrees with itself. These checks drive the real binary, so a product-side rename
or reclassification of a refusal code is caught here rather than turning a
declined write into a stopped run in the next benchmark. They skip when no
binary is built, which is the case on a runner that only checks out the
benchmark tree.

`admission_refused` is not exercised here: no committed profile makes this
build's gate refuse a `remember` turn, so the test would be asserting whatever
the gate happened to decide that day. The string is pinned by the product's own
tests instead (`crates/clio/src/cli_write_tests.rs` for the CLI mapping, and
`crates/clio/tests/retention_profile_harness.rs` for a refused store write).
"""

from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path

from benchmarks.clio_bench.adapters.clio import ClioAdapter, ClioAdapterError
from benchmarks.clio_bench.tokenizer import get_tokenizer
from benchmarks.clio_bench.types import REFUSAL_DUPLICATE, DialogueTurn

ROOT = Path(__file__).resolve().parents[2]
BINARY = ROOT / "target" / "debug" / "clio"
# A blank value counts as unset for the product's own environment overrides, so
# pinning these keeps the check away from a host key file and a host retention
# store, which would make the result depend on the machine.
_PINNED = {"CLIO_KEY_FILE": "", "CLIO_RETENTION_PROFILES": ""}
_TEXT = "Alice visited Paris in March."


def _real_binary() -> str | None:
    """Return the built clio binary, or None when this host has none.

    `CLIO_BENCH_CLIO_BINARY` names one directly, which a worktree without its
    own build directory uses to point at the checkout's binary.
    """
    explicit = os.environ.get("CLIO_BENCH_CLIO_BINARY")
    if explicit:
        return explicit if os.path.exists(explicit) else None
    return str(BINARY) if BINARY.exists() else None


class LiveWriteTests(unittest.TestCase):
    """Drive the real binary once for each way a `remember` call can end."""

    binary = _real_binary()

    def setUp(self) -> None:
        if self.binary is None:
            self.skipTest("no clio binary is built at target/debug/clio")
        self._tmp = tempfile.TemporaryDirectory(prefix="clio-bench-live-")
        self.addCleanup(self._tmp.cleanup)
        self.data_dir = os.path.join(self._tmp.name, "data")
        os.makedirs(self.data_dir, exist_ok=True)

    def _adapter(self, data_dir: str) -> ClioAdapter:
        return ClioAdapter(
            binary=self.binary,
            data_dir=data_dir,
            tokenizer=get_tokenizer(),
            extra_env=dict(_PINNED),
        )

    @staticmethod
    def _turn(text: str = _TEXT) -> DialogueTurn:
        return DialogueTurn(
            session_id="s1", turn_id="t1", speaker="user", role="user", text=text
        )

    def test_a_stored_turn_carries_no_refusal(self) -> None:
        adapter = self._adapter(self.data_dir)
        adapter.reset("s1")
        result = adapter.ingest("s1", self._turn())
        self.assertTrue(result.ok, result.error)
        self.assertIsNone(result.refusal)

    def test_a_repeated_turn_is_the_products_duplicate_decline(self) -> None:
        """`duplicate` is a decline, so it must stay a decline here."""
        adapter = self._adapter(self.data_dir)
        adapter.reset("s1")
        adapter.ingest("s1", self._turn())
        result = adapter.ingest("s1", self._turn())
        self.assertFalse(result.ok)
        self.assertEqual(result.refusal, REFUSAL_DUPLICATE)

    def test_a_relative_data_dir_is_a_failure_and_not_a_decline(self) -> None:
        """The issue #21 trigger: `config_corrupt` must not read as a refusal."""
        relative = os.path.relpath(self.data_dir, os.getcwd())
        self.assertFalse(os.path.isabs(relative), "the trigger must be relative")
        adapter = self._adapter(relative)
        adapter.reset("s1")
        with self.assertRaises(ClioAdapterError) as caught:
            adapter.ingest("s1", self._turn())
        self.assertIn("config_corrupt", str(caught.exception))


if __name__ == "__main__":
    unittest.main()
