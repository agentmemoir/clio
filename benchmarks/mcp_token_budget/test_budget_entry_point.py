#!/usr/bin/env python3
"""Drift guard: the budget job and the local entry point must agree.

The job runs one script, that script runs one check, and the README names the
script a reader is told to run. This test reads all three files and fails when
the chain breaks, so the documented local command and the CI command cannot
drift apart.
"""

import re
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "benchmarks.yml"
SCRIPT = REPO_ROOT / "scripts" / "mcp_budget_check.sh"
README = REPO_ROOT / "benchmarks" / "mcp_token_budget" / "README.md"

#: A workflow step that runs the entry point. A commented-out line never matches.
WORKFLOW_CALL = re.compile(r"^\s*run:\s*scripts/mcp_budget_check\.sh\s*$", re.MULTILINE)


class BudgetEntryPointTest(unittest.TestCase):
    """Assert the workflow call, the check call, and the README are still wired."""

    def test_workflow_runs_the_entry_point(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertRegex(workflow, WORKFLOW_CALL)

    def test_entry_point_runs_the_check(self):
        script = SCRIPT.read_text(encoding="utf-8")
        self.assertIn("benchmarks/mcp_token_budget/budget_check.py", script)

    def test_readme_names_the_entry_point(self):
        readme = README.read_text(encoding="utf-8")
        self.assertIn("scripts/mcp_budget_check.sh", readme)


if __name__ == "__main__":
    unittest.main()
