#!/usr/bin/env python3
"""clio-bench two-system baseline command-line entrypoint.

Run from the repository root, for example:

    python3 benchmarks/baseline.py --judge tier2 --generator tier2 \
        --out benchmarks/reports/baseline.json \
        --report-md benchmarks/reports/baseline.md

The command runs both real systems on the same pinned suite, partition, seed,
tokenizer, and judge. With no frontier key it writes the blocked-gate record
instead and claims no baseline.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from benchmarks.clio_bench.baseline import main  # noqa: E402

if __name__ == "__main__":
    raise SystemExit(main())
