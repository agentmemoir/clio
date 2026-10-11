"""Checks that a clippy invocation cannot quietly lint less than it claims.

Two commands in this repository describe themselves as workspace-wide: the
clippy line in the `ritual` make target, and the Clippy step in the pull request
workflow. Both used to name no package selector at all, and the root manifest set
`default-members` to a single crate, so cargo selected that one crate for both.
Sixteen of the seventeen crates, and their test targets, were never linted by
either command. A `-D warnings` error in any of them could not fail either gate,
and neither reported that it had looked at nothing.

The check exists because a lint gate that cannot reach a crate cannot report one,
and nothing else in the tree would have noticed.

Rules, each read where the thing it describes lives:

1. Every clippy invocation in the Makefile, in a shell script, or in a workflow
   names what it selects. `--workspace` does. So does `-p`. So does a `$(VAR)`
   whose every definition names a workspace token. A line is refused when it
   carries `--exclude`, because that drops a crate out of a workspace run while
   still reading as one. `--all` is accepted and reported: cargo documents it as
   a deprecated alias for `--workspace`, and a check that blesses the old spelling
   lets it come back without a word.
2. The root manifest does not set `default-members` inside `[workspace]`. It is
   the switch that made an unqualified `cargo clippy` mean one crate -- in a
   developer's shell as much as in CI -- and no command line can undo it
   everywhere.
3. At least one clippy run exists, and a file that mentions clippy but runs none
   is a finding. Deleting the Clippy step and the recipe both leave zero
   invocations, and a check that counts only the lines it can see reports that as
   clean, which is the loss this repository has already made once: a target that
   stopped running a check while keeping its name.
4. Every clippy run reaches cargo through a bare name, and nothing on the line
   names a toolchain around it. A toolchain binary invoked by path --
   `$(CARGO)`, `$(RUSTUP_BIN)/cargo`, or an absolute path -- does not read
   `rust-toolchain.toml`, so the run lints with whichever compiler this host
   carries while the runner uses the one the repository pins. Neither does a
   bare `cargo` that arrives through `rustup run <toolchain>`, or with
   `RUSTUP_TOOLCHAIN`, `RUSTC`, `RUSTDOC` or `CARGO` set on the same line:
   rustup resolves a named toolchain and its environment override ahead of the
   toolchain file, and the other three tell cargo which compiler to run. In
   every one of these shapes the two gates disagree about the same tree, and the
   local one is the wrong one. A `$(NAME)` or `${NAME}` reference is the one way
   to write the executable that can still pass: it passes when every definition
   of that name in the Makefile is a bare `cargo`, and is refused when one is a
   path or when the file never defines the name.

A file this check cannot read is not a skip. A scan that declines to look cannot
report what it did not see, and that is the failure this repository has had three
times already.

What is not scanned, stated rather than left to be assumed:

- **Python.** A clippy run added to a file under `scripts/` would not be caught
  here. It would be caught by `make ritual` and by CI, and a check that tried to
  find it by scanning Python source would read this file's own docstring and its
  own fixtures as two more invocations, so the false positives cost more than the
  hole.
- **`scripts/gate.py`,** which is the one file that narrows a clippy run on the
  direct-cargo path. It used to strip `--workspace` and substitute the changed
  crates, so a workspace-wide command a contributor typed produced a run over a
  few crates published as the lint verdict. It now returns a clippy command that
  names `--workspace` or `--all` unchanged, and that refusal is covered by the
  gate's own self-test rather than by this file. `make lint` is still scoped on
  purpose, through `PACKAGES`, and this check reads that variable rather than
  objecting to it.

Usage:
  python3 scripts/lint_scope_check.py
  python3 scripts/lint_scope_check.py --self-test
  python3 scripts/lint_scope_check.py --list-scanned

Exit codes:
  0  every clippy run names what it selects and reaches cargo by a bare name
     through the shim, with nothing on the line overriding the toolchain, the
     manifest does not narrow by default, and at least one clippy run exists
  1  at least one finding, each naming the file and line
  2  usage error
  3  a file or directory this check had to read could not be read, so the clean
     parts of the run do not cover it
"""

from __future__ import annotations

import sys
from pathlib import Path

from lint_scope_core import EXIT_OK, EXIT_USAGE, report, scan
from lint_scope_selftest import _self_test

USAGE = "usage: lint_scope_check.py | --self-test | --list-scanned"


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
