#!/usr/bin/env python3
"""Drift guard: the budget job and the local entry point must agree.

The job runs one script, that script runs one check, and the README names the
script a reader is told to run. This test reads all three files and fails when
the chain breaks, so the documented local command and the CI command cannot
drift apart.

It also pins the trigger. The check measures `skills/clio-mcp/SKILL.md`, so the
workflow must start when that file changes; otherwise the body can grow past
the frozen baseline and no job ever notices.
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

#: The shipped skill body, whose size the budget measures.
SKILL_PATH = "skills/clio-mcp/**"


def trigger_paths(workflow, event):
    """Return the `paths:` entries of one trigger, quoted or not."""
    lines = workflow.splitlines()
    try:
        start = lines.index(f"  {event}:")
    except ValueError:  # pragma: no cover - the trigger vanished
        return []
    entries = []
    in_paths = False
    for line in lines[start + 1:]:
        if line.startswith("  ") and not line.startswith("    "):
            break  # the next trigger key ends this one
        stripped = line.strip()
        if in_paths and stripped.startswith("- "):
            entries.append(stripped[2:].strip().strip("'\""))
        elif stripped == "paths:":
            in_paths = True
        elif in_paths:
            break
    return entries


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

    def test_push_paths_include_the_shipped_skill_body(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn(SKILL_PATH, trigger_paths(workflow, "push"))

    def test_pull_request_paths_include_the_shipped_skill_body(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn(SKILL_PATH, trigger_paths(workflow, "pull_request"))


if __name__ == "__main__":
    unittest.main()
