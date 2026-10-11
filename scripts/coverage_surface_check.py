"""Checks that a coverage run still measures one thing and reports it once.

The coverage surface carries four protections, and nothing in the tree stops a
later change from removing any of them:

- the instrumented run is split into a step that runs the tests and a step that
  writes the report, so the gate owns a boundary between them;
- a profile buffer too short to hold a raw profile header is deleted before the
  merge, and named when it is;
- an instrumented binary the run did not build is deleted before the merge, so
  the report cannot union a foreign object's rows into a file's numbers;
- a per-package measurement builds into an instrumented directory of its own.

This repository has already lost a make target's checks while keeping its name,
once, and `scripts/lint_scope_check.py` is the check whose whole purpose is to
catch that class. This file is the same kind of check for the coverage surface.
Every rule below is a configuration rule: it reads what the commands and the
commands' surroundings say, and it refuses a shape that has quietly stopped
meaning what it says. It runs no build and no coverage command.

Rules, each read where the thing it describes lives:

1. A coverage invocation that names fewer packages than the workspace, and takes
   its report from the shared instrumented directory, without either pinning an
   instrumented directory of its own or restricting the objects the merge may
   see, is a finding. A `--workspace` run is exempt, because it rebuilds every
   package and so holds no foreign binary. This is measured, not preferred: the
   coverage tool builds a report's object list by walking the instrumented
   target directory, and a package selector does not narrow that walk. Either
   protection is read from the file with its comments and its docstrings removed,
   so a sentence naming one is not one: this check exists to survive a later edit,
   and the edit that removes a protection is very often the sentence explaining
   that the protection was removed.
2. A coverage invocation that names fewer packages than the workspace and both
   runs the tests and writes the report in one command is a finding, and a run
   with no separate report step at all is the same finding, because the boundary
   this rule protects is the two steps and it is the only place the gate can drop
   a bad buffer or a foreign object. A report is written by a report flag or by
   `--no-run`, which the tool documents as generating a coverage report without
   running tests, so both count. A `--workspace` run is exempt from this rule and
   not from rule 3, for the reason in rule 1: what remains for it is the
   malformed buffer, and the failure mode is what answers that.
3. A coverage report step that does not ask the merge to fail only when every
   buffer is unreadable is a finding. That means no `--failure-mode` at all, and
   also `--failure-mode any`, which is the tool's default and behaves exactly as
   if the flag were absent: one malformed buffer ends the whole merge after every
   test has passed.
4. A function that writes a coverage report must drop the profile buffers it
   cannot read and delete the objects the run did not build, and both must happen
   before it writes the report, because the merge is what consumes them. This is
   the one rule here that is not about a command line, and it is here because the
   other three cannot see it: rule 1 is satisfied when a file names either
   protection, so deleting one of the two calls leaves the other to answer for it
   and the file still reads as protected.
5. A file this check must read and cannot read is never a pass, and neither is a
   file whose comments this reader cannot tell from its code. A scan that
   declines to look cannot report what it did not see, and a stripper that
   guessed would cut a command in half and hide the very invocation this check
   exists to report.

A file this check cannot read is not a skip, and neither is a builder it cannot
find. Both are reported: a missing file or a missing command builder is a finding
rather than a pass, because a Makefile that had been renamed, or a split that had
been undone, would otherwise turn this into a check of nothing.

What is not scanned, stated rather than left to be assumed:

- **Python other than `scripts/scoped_runner.py`.** The gate's own coverage
  commands are only ever expressed as Python list literals, so a check that
  ignored Python would find nothing to protect and this file would be the third
  copy of the same false negative. What is read is scoped to the two named
  command builders rather than grepped, because a grep over a Python file reads
  this file's own docstring and its own fixtures as coverage invocations, and the
  false positives cost more than the hole.
- **A protection marker is looked for in the whole file, not in the run.** A file
  that pins an instrumented directory of its own for one run and leaves another
  run in the same file unprotected is a false pass for rule 1. Telling them apart
  needs the run to be executed, and this check runs nothing. Rule 4 does not have
  that limit, because it reads the calls rather than the file.
- **A comment is not a protection, and neither is a docstring.** Comments and
  docstrings are removed before the markers are looked for, in the Makefile, in
  the shell scripts and in the workflows as well as in the runner, because a file
  that only mentions a protection reads as holding one. What is left out of the
  strip, and so still counts as a marker, is stated rather than assumed:
  - A `#` inside quotes, inside a `${NAME#prefix}` or `$(NAME)` expansion, after
    a `$`, or after a backslash is text, not a comment, and stays.
  - In a workflow a `#` starts a comment only at the start of a line or after a
    space, which is what YAML says, so `tag: v#1` keeps its value.
  - A line ending while a quote or an expansion is still open is kept whole. The
    quote began on an earlier line, and cutting at this line's `#` could drop a
    command that runs.
  - A `#` inside a backquote pair is a comment in the shell that reads it, so the
    line is cut there and nothing that runs is lost.
- **A file this reader cannot strip is refused, not judged.** Two things are
  refused: a file whose kind has no comment model here, and a file holding a
  here-document, whose body is data rather than commands and so has no `#` that
  starts a comment and no line that runs. Both are reported like a file that could
  not be read, and both fail closed. Neither is a silent skip: this repository
  holds no file of either kind today, so a refusal names a change to make.
- **A docstring written on the same line as its `def` is kept.** The reader takes
  a docstring to be the first statement of a block, and a string in a parameter
  list is a default value rather than prose, so the two cannot be told apart
  without parsing the signature. No file in this repository is written that way.
- **Rule 4 attributes a call to the function that writes it,** which means a
  hygiene step reached only through a helper called by two runs is seen once, at
  the helper, and the run that skipped it is not named. Reading a call graph
  needs a whole file, and this file reads two functions of one.
- **Rule 4 reads the stripped text, so a comment cannot answer for a hygiene step.**
  It used to read the raw line, which meant `pass  # prune_foreign_objects(...)`
  satisfied it: a run that had deleted both steps read as protected as long as
  someone left their names in a comment. Both readers now take the same stripped
  form, and the strip blanks prose instead of deleting it so a reported line number
  is still the line number in the source. A `#` inside a quoted string or a
  `${NAME#prefix}` expansion survives, because the strip tracks quoting and
  expansion rather than cutting at the first `#`.
- **Cargo aliases.** An alias defined in `.cargo/config.toml` or in the root
  manifest that expands to `llvm-cov` is invisible here, because nothing runs to
  expand it. A command line is read, not executed.
- **`scripts/gate.py`,** the one file that narrows a command on the direct-cargo
  path. It carries no coverage invocation to narrow: its coverage arm only names
  a kind. That is covered by the gate's own self-test rather than by this file.
- **Whether the protections work.** This reads configuration. It cannot show that
  the buffer scan finds every bad buffer, that the object rule keeps what the run
  needs, or that a report written this way is correct. The first two are the
  gate's own self-test cases; the third needs a real run, which this file refuses
  to perform because an instrumented build is expensive and shared.

Usage:
  python3 scripts/coverage_surface_check.py
  python3 scripts/coverage_surface_check.py --self-test
  python3 scripts/coverage_surface_check.py --list-scanned

Exit codes:
  0  every coverage invocation is a shape that can still measure one run and
     report it once, and both command builders are present
  1  at least one finding, each naming the file and line
  2  usage error
  3  a file or directory this check had to read could not be read, so the clean
     parts of the run do not cover it
"""

from __future__ import annotations

import sys
from pathlib import Path

from coverage_surface_core import EXIT_OK, EXIT_USAGE, report, scan
from coverage_surface_selftest import _self_test

USAGE = "usage: coverage_surface_check.py | --self-test | --list-scanned"


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return _self_test()
    if "-h" in argv or "--help" in argv:
        print(__doc__.strip())
        return EXIT_OK
    root = Path(__file__).resolve().parent.parent
    if "--list-scanned" in argv:
        for relative in scan(root).scanned:
            print(relative)
        return EXIT_OK
    if argv:
        print(USAGE, file=sys.stderr)
        return EXIT_USAGE
    return report(root)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
