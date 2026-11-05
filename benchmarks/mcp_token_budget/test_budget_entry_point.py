#!/usr/bin/env python3
"""Drift guard: the budget job and the local entry point must agree.

The job runs one script, and that script runs one check. This test reads both
files and fails when that chain breaks, so the documented local command and the
CI command cannot drift apart.
"""

import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "benchmarks.yml"
SCRIPT = REPO_ROOT / "scripts" / "mcp_budget_check.sh"


class BudgetEntryPointTest(unittest.TestCase):
    """Assert the workflow call and the check call are each still wired."""

    def test_workflow_runs_the_entry_point(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("run: scripts/mcp_budget_check.sh", workflow)

    def test_entry_point_runs_the_check(self):
        script = SCRIPT.read_text(encoding="utf-8")
        self.assertIn("benchmarks/mcp_token_budget/budget_check.py", script)


if __name__ == "__main__":
    unittest.main()
