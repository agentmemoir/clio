#!/usr/bin/env python3
"""clio-bench command-line entrypoint.

Run from the repository root, for example:

    python3 benchmarks/run.py --suite both --partition canary --max-probes 12 \
        --adapter clio --judge local --generator local \
        --out benchmarks/reports/run-clio.json --report-md benchmarks/reports/run-clio.md

The two-system baseline command owns the `baseline.json` / `baseline.md`
report paths; this entrypoint writes its own single-system reports elsewhere.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from benchmarks.clio_bench.cli import main  # noqa: E402

if __name__ == "__main__":
    raise SystemExit(main())
