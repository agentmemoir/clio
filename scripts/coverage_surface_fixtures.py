"""Fixture texts and the fixture builder for the coverage-surface self-test."""

from __future__ import annotations

from pathlib import Path

from coverage_surface_core import REPORT_BUILDER, RUNNER
from coverage_surface_text import MAKEFILE

# --- Self-test -----------------------------------------------------------------
#
# Every case is a fixture tree, so the cases describe the check rather than the
# repository and the suite gives the same answer on any machine. The fixtures live
# under one temporary directory that is removed when the suite returns. Nothing
# here is executed, so no fixture has to run.

#: The gate's two command builders and its two hygiene markers, in the shape the
#: real file has. Written out rather than produced by editing a copy of it,
#: because substring surgery on a fixture is how a case ends up testing a shape it
#: never built.
CLEAN_RUNNER = '''#!/usr/bin/env python3
"""A fixture gate."""


def coverage_test_command(packages):
    command = [
        "cargo", "llvm-cov", "nextest",
        *[flag for name in packages for flag in ("-p", name)],
        "--locked", "--no-report",
    ]
    return command


def coverage_report_command(report_path):
    return [
        "cargo", "llvm-cov", "report",
        "--json", "--output-path", str(report_path),
        "--failure-mode=all",
    ]


def run_coverage_tests(packages, env):
    return command


def write_coverage_report(report_path, env):
    return 0


def drop_invalid_profraws(directory):
    return 0, 0


def prune_foreign_objects(directory, selected):
    return 0


def run_coverage(packages, report_path, env):
    run_coverage_tests(packages, env)
    directory = env["dir"]
    drop_invalid_profraws(directory)
    prune_foreign_objects(directory, packages)
    write_coverage_report(report_path, env)


TARGET_DIR_KEY = "CARGO_LLVM_COV_TARGET_DIR"
'''

#: The gate's own run with the object deletion removed. Rule 1 still passes,
#: because the file still pins an instrumented directory of its own for the
#: per-package mode, so this is exactly the removal the other three rules cannot
#: see.
NO_OBJECT_DELETE_RUNNER = CLEAN_RUNNER.replace(
    "    prune_foreign_objects(directory, packages)\n", ""
)

#: The same run with the buffer scan removed.
NO_BUFFER_DROP_RUNNER = CLEAN_RUNNER.replace("    drop_invalid_profraws(directory)\n", "")

#: The same run with the object deletion removed and its name left behind in a
#: trailing comment. A reader sees the step named in the file; the run does not
#: do it. This is the shape a note left behind by an earlier edit takes, and it is
#: the reason rule 4 reads the stripped text rather than the line.
COMMENTED_OBJECT_DELETE_RUNNER = CLEAN_RUNNER.replace(
    "    prune_foreign_objects(directory, packages)\n",
    "    pass  # prune_foreign_objects(directory, packages)\n",
)

#: The same run, deleting the objects for real and also naming the step in a
#: trailing comment. Nothing here is wrong, so it must not become a finding: a
#: reader that strips too much would call this one, and a reader that strips
#: nothing would have passed the case above.
COMMENTED_AND_CALLED_RUNNER = CLEAN_RUNNER.replace(
    "    prune_foreign_objects(directory, packages)\n",
    "    prune_foreign_objects(directory, packages)  # and the report step needs it\n",
)

#: The same run with both steps moved below the report, which is a reordering
#: rather than a deletion and is just as invisible to a reader looking for calls.
HYGIENE_AFTER_REPORT_RUNNER = CLEAN_RUNNER.replace(
    "    drop_invalid_profraws(directory)\n"
    "    prune_foreign_objects(directory, packages)\n"
    "    write_coverage_report(report_path, env)\n",
    "    write_coverage_report(report_path, env)\n"
    "    drop_invalid_profraws(directory)\n"
    "    prune_foreign_objects(directory, packages)\n",
)

#: The whole-workspace diagnostic. `--workspace` makes rule 1 and rule 2 exempt by
#: measurement, so the failure mode is the whole of what rule 3 asks of it, and it
#: carries one.
CLEAN_MAKEFILE = """LLVM_COV_CMD := $(CARGO) llvm-cov nextest --workspace --locked --no-clean \\
\t--profile ci --failure-mode=all

coverage cov:
\t$(LLVM_COV_CMD) --json --output-path coverage.json
"""

#: A workflow that installs the tool and calls the make target. Neither is a
#: coverage command line, and a check that read them as one would report every
#: workflow that names the tool at all.
CLEAN_WORKFLOW = """on: [pull_request]
jobs:
  check:
    steps:
      - uses: taiki-e/install-action@v2
        with:
          tool: cargo-llvm-cov
      - name: Scoped coverage
        run: make coverage-fast BASE=origin/master
"""

#: The test step with a docstring that talks about the tool's `report`
#: subcommand, in prose, before the command it documents. Read without stripping
#: the prose, the word `report` is a subcommand the check finds and the step reads
#: as a report writer that passes no failure mode — two findings against the one
#: function that is exactly right.
PROSE_RUNNER = CLEAN_RUNNER.replace(
    "def coverage_test_command(packages):\n    command = [",
    "def coverage_test_command(packages):\n"
    '    """Build and run the tests, writing no report.\n\n'
    "    The tool documents `--no-run` as the way to generate a report\n"
    "    without running tests, so both that flag and the report subcommand\n"
    "    are named here and neither is passed to it.\n"
    '    """\n'
    "    command = [",
)

CLEAN_FILES = {
    RUNNER: CLEAN_RUNNER,
    MAKEFILE: CLEAN_MAKEFILE,
    ".github/workflows/ci.yml": CLEAN_WORKFLOW,
}

#: The test step with the tool's deprecated `--no-run`, which the tool documents as
#: generating a report without running the tests. It carries a failure mode, so
#: rule 3 has nothing to say and the case proves rule 2 alone.
NO_RUN_RUNNER = CLEAN_RUNNER.replace(
    '"--locked", "--no-report",', '"--locked", "--no-run", "--failure-mode=all",'
)

#: The test step writing the report itself. A separate report builder is still
#: present, so this fixture produces one finding and not two.
ONE_COMMAND_RUNNER = CLEAN_RUNNER.replace(
    '"--locked", "--no-report",',
    '"--locked", "--json", "--output-path", "coverage.json", "--failure-mode=all",',
)

#: The report step with no failure mode at all.
NO_MODE_RUNNER = CLEAN_RUNNER.replace('"--failure-mode=all",\n', "")

#: The report step asking for the tool's default, which behaves as if the flag
#: were absent.
ANY_MODE_RUNNER = CLEAN_RUNNER.replace('"--failure-mode=all",', '"--failure-mode=any",')

#: The failure mode in the other spelling, so a check that read only one of them
#: would report a command that does pass one as one that does not.
SPACED_MODE_RUNNER = CLEAN_RUNNER.replace(
    '"--failure-mode=all",', '"--failure-mode", "all",'
)

#: One command in place of the two steps, which is the shape rule 2 exists for.
ONE_BUILDER_RUNNER = '''#!/usr/bin/env python3
"""A fixture gate."""


def coverage_test_command(packages):
    command = [
        "cargo", "llvm-cov", "nextest",
        *[flag for name in packages for flag in ("-p", name)],
        "--locked", "--json", "--output-path", "coverage.json",
        "--failure-mode=all",
    ]
    return command


def prune_foreign_objects(directory, selected):
    return 0
'''

def _without_report_builder(text: str) -> str:
    """The fixture with its report step removed and nothing else changed.

    Cut out by name rather than by truncating at the point where the report step
    starts, because a truncation also takes the hygiene functions with it and the
    test step then looks unprotected. That produces a second finding, and the case
    would pass with the wrong rule firing.
    """
    start = text.index(f"def {REPORT_BUILDER}")
    return text[:start] + text[text.index("def drop_invalid_profraws") :]


#: A per-package *report* step in a protected file. It names a package and it
#: writes a report, but it runs no tests, so rule 2 must not fire on it: a check
#: that read every command naming `-p` and a report flag as one command doing both
#: would fire here, and a later change adding a package filter to the report step
#: would be refused for a boundary it never crossed.
SUBSET_REPORT_MAKEFILE = """CARGO_LLVM_COV_TARGET_DIR := target/llvm-cov-package/clio-store

coverage cov:
\tcargo llvm-cov report -p clio-store --json --output-path coverage.json --failure-mode=all
"""

#: The same, with no subcommand named. That is the tool's default, and its
#: documented default runs the tests before writing the report, so this line runs
#: tests and writes a report in one command. A check that looked for the words
#: `nextest` or `test` and found neither would read it as a report step alone and
#: say nothing.
NO_SUBCOMMAND_BOUNDARY = """CARGO_LLVM_COV_TARGET_DIR := target/llvm-cov-package/clio-store

coverage cov:
\tcargo llvm-cov -p clio-store --json --output-path coverage.json --failure-mode=all
"""

#: A line naming both selectors. `--workspace` is read first, because a package
#: selector beside it widens the run back to every package rather than narrowing
#: it, and refusing that line would refuse a workspace-wide run for carrying a
#: selector it does not act on.
WORKSPACE_AND_PACKAGE = """COV_CMD := $(CARGO) llvm-cov nextest --workspace -p clio-store --locked --no-report

coverage cov:
\t$(COV_CMD)
"""

#: A per-package command in the Makefile with nothing to keep the merge inside the
#: packages it selected.
UNPROTECTED_MAKEFILE = """coverage cov:
\tcargo llvm-cov nextest -p clio-store --locked --no-report --failure-mode=all
"""

#: The same command in a Makefile that pins an instrumented directory of its own,
#: which is the other way this repository has answered the foreign-object problem.
PINNED_MAKEFILE = """CARGO_LLVM_COV_TARGET_DIR := target/llvm-cov-package/clio-store
COV_CMD := $(CARGO) llvm-cov nextest -p clio-store --locked --no-report

coverage cov:
\t$(CARGO) llvm-cov report --json --output-path coverage.json --failure-mode=all
"""

#: A whole-workspace command doing both in one invocation. Exempt from rule 2 by
#: measurement and by the reason rule 1 gives, and so a case rather than a hole.
WORKSPACE_ONE_COMMAND = """COV_CMD := $(CARGO) llvm-cov nextest --workspace --locked

coverage cov:
\t$(COV_CMD) --json --output-path coverage.json --failure-mode=all
"""

#: The two steps of a per-package run, as the Makefile writes them. Used on its own
#: by the comment case below, and by the shell and workflow cases that wrap it.
SUBSET_STEPS = (
    "COV_CMD := $(CARGO) llvm-cov nextest -p clio-store --locked --no-report\n"
    "\n"
    "coverage cov:\n"
    "\t$(COV_CMD)\n"
    "\t$(CARGO) llvm-cov report --json --output-path coverage.json --failure-mode=all\n"
)

#: A marker that only a comment carries. The comment is the sentence a person
#: writes while removing a protection, and it names the variable the removal
#: dropped, so read as a marker it silences the very rule that would report the
#: removal. Before the fix this file was judged protected and exited clean.
COMMENTED_MARKER_MAKEFILE = (
    "# the old approach set CARGO_LLVM_COV_TARGET_DIR by hand\n" + SUBSET_STEPS
)

#: The same run with the marker on a line that pins it, and a trailing comment on
#: that line, so a stripper that cut too eagerly fails here instead of passing.
PINNED_WITH_COMMENT_MAKEFILE = (
    "CARGO_LLVM_COV_TARGET_DIR := target/llvm-cov-package/clio-store"
    "  # one directory per package, not shared\n" + SUBSET_STEPS
)

#: A marker carried by a comment in a shell script rather than a Makefile.
COMMENTED_MARKER_SCRIPT = (
    "#!/bin/sh\n"
    "# an older script set CARGO_LLVM_COV_TARGET_DIR itself\n"
    "cargo llvm-cov nextest -p clio-store --locked --no-report\n"
    "cargo llvm-cov report --json --output-path coverage.json --failure-mode=all\n"
)

#: A marker carried by a comment in a workflow. The `#` sits after a space, so it
#: starts a comment by YAML's own rule and a stripper that cut at every `#` would
#: find the same thing.
COMMENTED_MARKER_WORKFLOW = (
    CLEAN_WORKFLOW
    + "      # this step used to export CARGO_LLVM_COV_TARGET_DIR\n"
    + "      - name: One crate\n"
    + "        run: cargo llvm-cov nextest -p clio-store --locked --no-report\n"
)

#: A `#` that is not a comment, with the command after it on the same line. The
#: `${NAME#prefix}` expansion strips a prefix from a value, so the character is
#: part of the name; the quoted `#` is part of a value. A stripper that cut at
#: either one would truncate the line, and a truncated line reads as no command at
#: all, which is a run this check cannot report.
EXPANSION_HASH_MAKEFILE = (
    "coverage cov:\n"
    "\tOUT=$${OUT#-} ; cargo llvm-cov nextest -p clio-store --locked --no-report\n"
)

QUOTED_HASH_MAKEFILE = (
    'coverage cov:\n'
    '\tOUT="dir#one" ; cargo llvm-cov nextest -p clio-store --locked --no-report\n'
)

#: A here-document, whose body is data rather than commands. No `#` in it starts a
#: comment and no line in it runs, so a reader that cannot tell the body from the
#: script must refuse the file rather than judge it.
HEREDOC_MAKEFILE = """coverage cov:
\tprintf '%s\\n' "$(cat <<'NOTES'
CARGO_LLVM_COV_TARGET_DIR is set per package, not by hand
NOTES
)"
\tcargo llvm-cov nextest -p clio-store --locked --no-report
"""


def _fixture(base: Path, label: str, changes: dict[str, str | None] | None = None) -> Path:
    """One case's tree: the clean files, with `changes` applied over them.

    The overrides are a dict of real relative paths rather than keyword arguments,
    because a keyword cannot carry the slash in `scripts/scoped_runner.py`.
    """
    files = dict(CLEAN_FILES)
    for relative, body in (changes or {}).items():
        if body is None:
            files.pop(relative, None)
        else:
            files[relative] = body
    root = base / label
    for relative, text in files.items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    return root
