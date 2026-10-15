#!/usr/bin/env python3
"""Quality harness for extraction admission and span grounding.

The harness has two deliberately separate inputs:

* ``extract_fixtures.json`` is a frozen synthetic tuning set.  It also has
  small negative controls which preserve the recorded pre-fix empty response.
* ``extract_heldout.json`` is a separately frozen synthetic set.  Its expected
  values are authored from the construction grammar, never copied from a live
  response.

The default run executes the local unit set.  A live request is attempted only
when ``--endpoint`` is supplied.  Use ``--unit-only`` to make that intent
explicit, or ``--live-only`` to require a live run.  No live result is ever
synthesised when an endpoint is absent or unavailable.  ``--parity`` evaluates
``verifier_parity.json`` locally and emits per-case booleans without making a
request.

Fixture format (version 1)::

    {
      "format": "clio-extract-fixture-v1",
      "set": "tuning" | "heldout",
      "frozen": true,
      "construction": "why and when this synthetic set was frozen",
      "cases": [
        {
          "id": "unique-public-case-id",
          "class": "synthetic_class",
          "source_text": "synthetic source",
          "schema": {"field": ""},
          "fields": [
            {"path": "field", "kind": "entity|number|date", "required": true}
          ],
          "expected_snapshot": {"field": "exact value"},
          "gold": {"field": "exact value"},
          "expected_admission": true,
          "expected_grounding": true,
          "raw_response": {"snapshot": {}, "gist": "synthetic"},
          "baseline_raw_response": {"snapshot": {}}
        }
      ]
    }

``expected_admission`` and ``expected_grounding`` describe what the recorded
response must do.  Negative controls therefore retain a fixed expected
snapshot while asserting that their historical response is rejected.  The
span verifier is applied to every declared non-empty field, and non-empty
unlisted leaves are rejected fail-closed; an expected snapshot never makes an
ungrounded value acceptable.

Only Python's standard library is used.  Live HTTP is explicit, bounded,
time-limited, and never prints a response body.  ``--raw-output`` optionally
stores a redacted, bounded diagnostic record; it is not required for unit
mode.
"""

from __future__ import annotations

import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from extract_quality_candidate import parse_candidate_response  # noqa: E402
from extract_quality_cli import main  # noqa: E402
from extract_quality_core import (  # noqa: E402
    DEFAULT_MIN_FIDELITY,
    DEFAULT_MODEL,
    DEFAULT_TEMPERATURE,
    DEFAULT_TIMEOUT,
    EXIT_FIXTURE_ERROR,
    EXIT_OK,
    EXIT_QUALITY_FAIL,
    EXIT_USAGE,
    HELDOUT_FIXTURE,
    MAX_FIXTURE_BYTES,
    MAX_RAW_ITEMS,
    MAX_RAW_STRING_CHARS,
    MAX_REQUEST_BYTES,
    MAX_RESPONSE_BYTES,
    MAX_TIMEOUT,
    SCRIPT_DIR,
    TUNING_FIXTURE,
    VERIFIER_PARITY,
    Candidate,
    CaseResult,
    FieldSpec,
    FixtureCase,
    FixtureSet,
    HarnessError,
    SuiteReport,
    VerifyConfig,
    VerifyReport,
    format_causes,
)
from extract_quality_evaluate import run_unit  # noqa: E402
from extract_quality_fixture import load_fixture  # noqa: E402
from extract_quality_live import (  # noqa: E402
    COMPLETION_INSTRUCTIONS,
    SYSTEM_PROMPT,
    build_request,
    build_user_message,
    run_live,
)
from extract_quality_redact import redact_json  # noqa: E402
from extract_quality_span import json_path, nfc, verify_snapshot  # noqa: E402

__all__ = [
    "DEFAULT_MIN_FIDELITY",
    "DEFAULT_MODEL",
    "DEFAULT_TEMPERATURE",
    "DEFAULT_TIMEOUT",
    "EXIT_FIXTURE_ERROR",
    "EXIT_OK",
    "EXIT_QUALITY_FAIL",
    "EXIT_USAGE",
    "HELDOUT_FIXTURE",
    "MAX_FIXTURE_BYTES",
    "MAX_RAW_ITEMS",
    "MAX_RAW_STRING_CHARS",
    "MAX_REQUEST_BYTES",
    "MAX_RESPONSE_BYTES",
    "MAX_TIMEOUT",
    "SCRIPT_DIR",
    "TUNING_FIXTURE",
    "VERIFIER_PARITY",
    "COMPLETION_INSTRUCTIONS",
    "SYSTEM_PROMPT",
    "Candidate",
    "CaseResult",
    "FieldSpec",
    "FixtureCase",
    "FixtureSet",
    "HarnessError",
    "SuiteReport",
    "VerifyConfig",
    "VerifyReport",
    "format_causes",
    "json_path",
    "nfc",
    "parse_candidate_response",
    "redact_json",
    "verify_snapshot",
    "build_request",
    "build_user_message",
    "load_fixture",
    "run_live",
    "run_unit",
    "main",
]

if __name__ == "__main__":
    sys.exit(main())
