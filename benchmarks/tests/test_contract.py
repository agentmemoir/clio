"""Adapter contract tests: every registered adapter honours the interface.

The adapter set is read from the runner's registry, so an adapter cannot be
selectable by one command and untested by this suite. Each registered adapter
is then driven for behaviour over a hermetic stand-in system, not merely
inspected for method names.
"""

from __future__ import annotations

import os
import shutil
import stat
import sys
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))

from exec_tempdir import exec_tempdir  # noqa: E402  (path set above)

from benchmarks.clio_bench.adapter import MemoryBenchmarkAdapter  # noqa: E402
from benchmarks.clio_bench.cli import build_parser  # noqa: E402
from benchmarks.clio_bench.runner import (  # noqa: E402
    ADAPTER_BUILDERS,
    ADAPTER_DEFAULT_BINARIES,
    ADAPTER_NAMES,
)
from benchmarks.clio_bench.tokenizer import get_tokenizer  # noqa: E402
from benchmarks.clio_bench.types import DialogueTurn  # noqa: E402
from benchmarks.tests.support import (  # noqa: E402
    FAKE_SYSTEM,
    assert_context_accounting,
)


def _turn(session_id: str, turn_id: str, text: str) -> DialogueTurn:
    return DialogueTurn(
        session_id=session_id,
        turn_id=turn_id,
        speaker="user",
        role="user",
        text=text,
    )


class RegistryDrivenContractTests(unittest.TestCase):
    """Every registered adapter, built the way the runner builds it."""

    def setUp(self) -> None:
        self._tmp = exec_tempdir(purpose="the fake system binary")
        self.addCleanup(self._tmp.cleanup)
        self.tmpdir = self._tmp.name
        self.fake = os.path.join(self.tmpdir, "fake_system.py")
        with open(self.fake, "w", encoding="utf-8") as handle:
            handle.write(FAKE_SYSTEM)
        os.chmod(self.fake, os.stat(self.fake).st_mode | stat.S_IXUSR)
        self.tokenizer = get_tokenizer()
        self.state = os.path.join(self.tmpdir, "stand-in-store")
        os.makedirs(self.state)

    def build(self, name: str, **overrides: str) -> MemoryBenchmarkAdapter:
        """Build one registered adapter with a hermetic stand-in behind it.

        The options come from the real parser, so a name the registry accepts is
        a name the command line accepts and the adapter is constructed by the
        same code path a run uses.
        """
        options = build_parser().parse_args(
            [
                "--adapter",
                name,
                "--binary",
                overrides.get("binary", self.fake),
                "--data-dir",
                overrides.get("data_dir", os.path.join(self.tmpdir, "data")),
            ]
        )
        return ADAPTER_BUILDERS[name](self.tokenizer, options)

    # -- the registry is the test set ------------------------------------

    def test_every_registered_adapter_is_selectable_and_buildable(self) -> None:
        parser = build_parser()
        for name in ADAPTER_NAMES:
            with self.subTest(adapter=name):
                options = parser.parse_args(["--adapter", name])
                adapter = ADAPTER_BUILDERS[name](self.tokenizer, options)
                self.assertIsInstance(adapter, MemoryBenchmarkAdapter)

    def test_the_cli_choice_list_is_the_registry(self) -> None:
        action = next(
            action
            for action in build_parser()._actions
            if action.dest == "adapter"
        )
        self.assertEqual(list(action.choices), list(ADAPTER_NAMES))

    def test_each_adapter_without_a_binary_drives_its_own_executable(self) -> None:
        """Absent --binary means that adapter's own system, for every adapter.

        The flag used to carry one default for all adapters, which is why a
        command selecting a rival could not name its executable. This asserts the
        resolution against the runner's documented map, and that an explicit
        --binary still wins, so the default cannot quietly swallow the flag.

        The resolved executable is read from the built adapter's private field
        because the interface exposes no accessor for it and the runners own
        construction; the same reach is already used in this suite's sibling
        test module.
        """
        parser = build_parser()
        self.assertEqual(
            set(ADAPTER_DEFAULT_BINARIES) - set(ADAPTER_NAMES),
            set(),
            "a default executable is documented for an unregistered adapter",
        )
        for name in ADAPTER_NAMES:
            with self.subTest(adapter=name):
                options = parser.parse_args(["--adapter", name])
                self.assertIsNone(options.binary)
                adapter = ADAPTER_BUILDERS[name](self.tokenizer, options)
                if not hasattr(adapter, "_binary"):
                    # A double that runs in this process has no executable, so it
                    # has nothing to default and must not claim one.
                    self.assertNotIn(name, ADAPTER_DEFAULT_BINARIES)
                    continue
                self.assertEqual(adapter._binary, ADAPTER_DEFAULT_BINARIES.get(name))

                chosen = ADAPTER_BUILDERS[name](
                    self.tokenizer,
                    parser.parse_args(["--adapter", name, "--binary", "/one/chosen/binary"]),
                )
                self.assertEqual(chosen._binary, "/one/chosen/binary")

    def test_every_registered_adapter_exposes_the_interface_methods(self) -> None:
        for name in ADAPTER_NAMES:
            with self.subTest(adapter=name):
                adapter = self.build(name)
                for method in (
                    "reset",
                    "ingest",
                    "retrieve",
                    "answer",
                    "healthcheck",
                    "fingerprint",
                ):
                    self.assertTrue(callable(getattr(adapter, method, None)), method)

    def test_every_registered_adapter_reports_the_fingerprint_shape(self) -> None:
        """The fingerprint shape is one contract, so it is asserted once.

        Every registered adapter is driven through the same stand-in, so an
        adapter that cannot state its settings or records an empty object fails
        here without this file knowing which system it is.
        """
        for name in ADAPTER_NAMES:
            with self.subTest(adapter=name):
                fingerprint = self.build(name).fingerprint()
                payload = fingerprint.to_json()
                self.assertEqual(set(payload), {"system", "version", "settings"})
                self.assertTrue(payload["system"])
                self.assertTrue(payload["version"])
                self.assertIsInstance(payload["settings"], dict)
                self.assertTrue(
                    payload["settings"],
                    "a fingerprint must record a setting or an explicit absence",
                )
                for field, record in payload["settings"].items():
                    self.assertIn(record["status"], ("read", "unreadable"), field)
                    if record["status"] == "read":
                        self.assertIn("surface", record)
                        self.assertNotIn("tried", record)
                    else:
                        self.assertTrue(record["tried"], field)
                        self.assertNotIn("value", record)
                fingerprint.validate()

    # -- behaviour, not presence ----------------------------------------

    def stand_in(self, name: str) -> str:
        """Return an empty stand-in store dedicated to one adapter.

        Every adapter derives the same bank name from a session, so a shared
        store would let one adapter's write answer another's recall.
        """
        path = os.path.join(self.state, name)
        shutil.rmtree(path, ignore_errors=True)
        os.makedirs(path)
        return path

    def test_every_registered_adapter_completes_a_round_trip(self) -> None:
        for name in ADAPTER_NAMES:
            with self.subTest(adapter=name), mock.patch.dict(
                os.environ, {"CLIO_BENCH_FAKE_STATE": self.stand_in(name)}
            ):
                adapter = self.build(name)
                adapter.reset("s1")
                self.assertTrue(adapter.healthcheck())

                ingested = adapter.ingest("s1", _turn("s1", "t1", "Alice visited Paris."))
                self.assertTrue(ingested.ok)
                self.assertIsNone(ingested.error)
                self.assertEqual(
                    ingested.tokens_processed, self.tokenizer.count("Alice visited Paris.")
                )

                retrieval = adapter.retrieve("s1", "Where did Alice go?", 4096)
                self.assertIn("Paris", retrieval.context)
                self.assertEqual(len(retrieval.item_ids), 1)
                self.assertGreaterEqual(retrieval.latency_ms, 0.0)

    # A refusal path is not asserted here. It is a property of a system that has
    # an admission gate, and every registered adapter can be driven hermetically,
    # so the contract owes them the round trip above instead. The refusal path of
    # each CLI-backed system is covered by that adapter's own suite.

    def test_every_registered_adapter_keeps_a_tight_budget(self) -> None:
        """The composed context stays inside the budget, for every adapter.

        The budget is the real cost of the first line this adapter actually
        packs, so the assertion measures the context the harness would hand to a
        model rather than the adapter's own running total, which is the number
        that can disagree with it. A literal line written for one system would be
        the per-adapter knowledge this file must not hold, and it would not fit
        another adapter's line shape.
        """
        for name in ADAPTER_NAMES:
            with self.subTest(adapter=name), mock.patch.dict(
                os.environ, {"CLIO_BENCH_FAKE_STATE": self.stand_in(name)}
            ):
                adapter = self.build(name)
                adapter.reset("s1")
                first = adapter.ingest("s1", _turn("s1", "t1", "alpha beta gamma"))
                second = adapter.ingest("s1", _turn("s1", "t2", "delta epsilon"))
                self.assertTrue(first.ok)
                self.assertTrue(second.ok)

                wide = adapter.retrieve("s1", "alpha", 4096)
                self.assertTrue(wide.context, f"{name} packed nothing to budget against")
                first_line = wide.context.split("\n")[0]
                budget = self.tokenizer.count(first_line)

                retrieval = adapter.retrieve("s1", "alpha", budget)
                self.assertEqual(retrieval.context, first_line)
                self.assertLessEqual(self.tokenizer.count(retrieval.context), budget)
                self.assertLessEqual(retrieval.tokens_retrieved, budget)
                self.assertEqual(len(retrieval.item_ids), 1)

    def test_every_registered_adapter_reports_the_cost_of_its_context(self) -> None:
        """`tokens_retrieved` is the cost of `context`, for every adapter.

        The budget is wide, so each adapter packs everything its own recall
        returns. Whether that is one hit or two is that adapter's retrieval
        behaviour and is not something the interface requires, so the invariant
        is asserted on whatever was returned. The separator and prefix cases are
        asserted where the shape is known: in `test_packing` for the rule, and
        in each adapter's own suite for its own line shape.
        """
        for name in ADAPTER_NAMES:
            with self.subTest(adapter=name), mock.patch.dict(
                os.environ, {"CLIO_BENCH_FAKE_STATE": self.stand_in(name)}
            ):
                adapter = self.build(name)
                adapter.reset("s1")
                first = adapter.ingest("s1", _turn("s1", "t1", "alpha beta gamma"))
                second = adapter.ingest("s1", _turn("s1", "t2", "delta epsilon"))
                self.assertTrue(first.ok)
                self.assertTrue(second.ok)

                retrieval = adapter.retrieve("s1", "alpha delta", 4096)

                self.assertTrue(retrieval.context, f"{name} packed nothing")
                assert_context_accounting(self, retrieval, self.tokenizer)
                self.assertLessEqual(retrieval.tokens_retrieved, 4096)

    def test_every_registered_adapter_isolates_sessions(self) -> None:
        for name in ADAPTER_NAMES:
            with self.subTest(adapter=name), mock.patch.dict(
                os.environ, {"CLIO_BENCH_FAKE_STATE": self.stand_in(name)}
            ):
                adapter = self.build(name)
                adapter.reset("alpha")
                adapter.ingest("alpha", _turn("alpha", "t1", "only in alpha"))
                adapter.reset("beta")
                adapter.ingest("beta", _turn("beta", "t1", "only in beta"))

                self.assertIn("alpha", adapter.retrieve("alpha", "alpha", 4096).context)
                self.assertNotIn("alpha", adapter.retrieve("beta", "alpha", 4096).context)

    # -- the interface itself -------------------------------------------

    def test_abstract_adapter_cannot_be_instantiated(self) -> None:
        with self.assertRaises(TypeError):
            MemoryBenchmarkAdapter()  # type: ignore[abstract]

if __name__ == "__main__":
    unittest.main()