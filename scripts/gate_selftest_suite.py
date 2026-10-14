"""The runner the gate's `--self-test` entry point calls, the environment it pins, and the check that the pin is complete."""

from __future__ import annotations

import io
import os
import tempfile
import unittest
from unittest import mock

from gate_kinds import ENV_KEY_EXEMPT, GATE_SCOPE_DEFAULT

#: The environment the suite pins so the caller's exports cannot change what the
#: tests exercise. Every value this repository reads but does not key
#: (`ENV_KEY_EXEMPT`) must appear here; a self-test enforces that pair, because
#: the capture switch once landed without its pin and nine tests changed meaning
#: under an exported value.
SUITE_ENV = {
    "XDG_CACHE_HOME": "",
    "CLIO_GATE_LOG_DIR": "",
    "CLIO_GOLDEN_CAPTURE": "",
    "GATE_ACTIVE": "",
    "GATE_FORCE": "",
    "GATE_RAW": "",
    "GATE_SCOPE": GATE_SCOPE_DEFAULT,
    "MAKEFLAGS": "",
    "GNUMAKEFLAGS": "",
    "BASE": "",
    "PACKAGES": "",
}


class SuiteEnvironmentTests(unittest.TestCase):
    def test_every_read_but_unkeyed_value_is_pinned(self):
        # The other half of the env-read rule that gate_selftest_a states for
        # the key: a value this repository reads but does not key must also be
        # pinned here, or the caller's export changes what a test exercises.
        # The capture switch landed without its pin and nine tests changed
        # meaning under an exported value.
        for name in sorted(ENV_KEY_EXEMPT):
            self.assertIn(name, SUITE_ENV, f"{name} is read but not keyed, and the suite does not pin it")


def run_suite() -> int:
    """Run every gate self-test module and return the process exit code."""
    import gate_selftest_a
    import gate_selftest_b
    import gate_selftest_c
    import gate_selftest_d
    import gate_selftest_e
    import gate_selftest_f
    import gate_selftest_g
    import gate_selftest_h
    import gate_selftest_i

    loader = unittest.TestLoader()
    suite = unittest.TestSuite()
    suite.addTests(loader.loadTestsFromTestCase(SuiteEnvironmentTests))
    for mod in (
        gate_selftest_a,
        gate_selftest_b,
        gate_selftest_c,
        gate_selftest_d,
        gate_selftest_e,
        gate_selftest_f,
        gate_selftest_g,
        gate_selftest_h,
        gate_selftest_i,
    ):
        suite.addTests(loader.loadTestsFromModule(mod))
    buffer = io.StringIO()
    with tempfile.TemporaryDirectory(prefix="gate-self-test-") as scratch:
        with mock.patch.dict(os.environ, {**SUITE_ENV, "XDG_CACHE_HOME": scratch}):
            result = unittest.TextTestRunner(stream=buffer, verbosity=1).run(suite)
    skipped = [f"{test}: {reason}" for test, reason in getattr(result, "skipped", [])]
    if not result.wasSuccessful():
        print(buffer.getvalue())
    for reason in skipped:
        print(f"self-test skipped: {reason}")
    clean = result.wasSuccessful()
    if not clean:
        summary = "FAILED"
    elif skipped:
        summary = f"all passed, {len(skipped)} check group(s) skipped"
    else:
        summary = "all passed"
    print(f"self-test: {summary}")
    return 0 if clean else 1
