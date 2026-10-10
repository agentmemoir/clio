from __future__ import annotations

from pathlib import Path

from gate_kinds import (
    CARGO_KINDS,
    LLVM_COV_KNOWN_SUBCOMMANDS,
    LLVM_COV_LOOKUP_KIND,
    LLVM_COV_PROBE_FLAGS,
    LLVM_COV_REPORTING_SUBCOMMANDS,
    MAKE_KINDS,
    NEXTEST_EXECUTING_SUBCOMMANDS,
    NEXTEST_KNOWN_SUBCOMMANDS,
    NEXTEST_LOOKUP_KIND,
)

def nextest_subcommand_index(argv: list[str]) -> int | None:
    """Where the `cargo nextest` subcommand sits in argv, or None if unreadable.

    nextest puts its own options before the subcommand, so the token straight
    after `nextest` is not always the subcommand, and `cargo nextest --verbose
    run` is a real command that runs every test in the workspace. Reading one
    token is wrong for it; knowing which options swallow a token is wrong as
    soon as the tool grows one.

    So the walk looks for a token this file recognises as a subcommand and skips
    everything else. An option's value and a filter argument are both skipped
    without needing to tell them apart, because neither can be mistaken for
    something the walk is looking for.

    None is returned rather than a guess when no subcommand can be found. An
    unreadable command must not be reported as a test run: that is the mistake
    this whole classification exists to stop. A filter past `--` may be spelled
    like a subcommand, so the walk stops there.
    """
    for index in range(2, len(argv)):
        token = argv[index]
        if token == "--":
            return None
        if token in NEXTEST_KNOWN_SUBCOMMANDS:
            return index
    return None


def nextest_subcommand(argv: list[str]) -> str:
    """The `cargo nextest` subcommand, or "" when the command line does not say."""
    index = nextest_subcommand_index(argv)
    return argv[index] if index is not None else ""


def llvm_cov_subcommand_index(argv: list[str]) -> int | None:
    """Where the `cargo llvm-cov` subcommand sits in argv, or None if unreadable.

    llvm-cov writes `cargo llvm-cov [SUBCOMMAND] [OPTIONS]`, so its subcommand is
    meant to come first, but an option may precede it and the walk here is the
    same one `nextest_subcommand_index` uses: skip anything this file does not
    recognise and take the first token that is a subcommand. That way an option
    added upstream, or one taking a value this file has never heard of, cannot
    hide the subcommand behind it.

    The walk stops at `--`, because everything past it is an argument for the
    test binary and a filter is allowed to be spelled exactly like a subcommand.

    None is returned rather than a guess when no subcommand can be found. A bare
    `cargo llvm-cov` is `cargo llvm-cov test` in every way this gate cares about,
    so an unreadable line is left to the caller to judge as a coverage run
    rather than being reported as a probe.
    """
    for index in range(2, len(argv)):
        token = argv[index]
        if token == "--":
            return None
        if token in LLVM_COV_KNOWN_SUBCOMMANDS:
            return index
    return None


def llvm_cov_measures_coverage(argv: list[str]) -> bool:
    """Whether this `cargo llvm-cov` line produces a coverage report.

    Three shapes measure and two do not. A subcommand that measures, and a bare
    `cargo llvm-cov` with no subcommand at all, both produce a report: the bare
    form is documented as equivalent to `cargo llvm-cov test`. Everything else
    does not: `show-env` prints the environment, `clean` removes the artifacts,
    and a self-describing flag asks the tool about itself.

    A flag that describes the tool is checked across the whole line rather than
    only when no subcommand was found, because the two can both be present:
    `cargo llvm-cov --version report` asks the tool its version, writes no report
    and exits 0, and reading the subcommand first would publish that as a
    coverage verdict. The scan stops at `--`, because a filter past it belongs to
    the test binary, so a token spelled like a flag there is an argument.

    The limit, stated rather than hidden: a subcommand this file has never heard
    of reads as a bare run, so it is classified as a coverage run. That is the
    opposite of the nextest rule, and deliberately so. `cargo nextest` alone is an
    error, so an unrecognised token there can only be a subcommand this file has
    not caught up with. `cargo llvm-cov` alone is a coverage run, and its options
    take bare values -- `--output-path reports/out.json` is a report, not a
    subcommand -- so treating any unrecognised bare token as a subcommand would
    classify real reports as probes and lose their artifacts. A self-test
    compares the subcommand list against the installed tool, so an upstream
    addition fails a check rather than being silently misclassified.
    """
    for token in argv[2:]:
        if token == "--":
            break
        if token in LLVM_COV_PROBE_FLAGS:
            return False
    index = llvm_cov_subcommand_index(argv)
    if index is not None:
        return argv[index] in LLVM_COV_REPORTING_SUBCOMMANDS
    return True


def command_kind(argv: list[str]) -> str:
    base = Path(argv[0]).name
    if base == "make":
        for arg in argv[1:]:
            if arg in MAKE_KINDS:
                return MAKE_KINDS[arg]
        return "make"
    if base == "cargo" and len(argv) > 1:
        if argv[1] == "nextest":
            # What the tool was asked to do, not which tool was called. Only
            # `run` executes the test suite, and the pre-commit hook runs the
            # listing one, so classifying by tool alone let a listing stand in
            # for a run's verdict. A subcommand this file does not know lands
            # here too, which is the safe side: it is published under its own
            # name rather than claiming to be a test run.
            if nextest_subcommand(argv) in NEXTEST_EXECUTING_SUBCOMMANDS:
                return CARGO_KINDS["nextest"]
            return NEXTEST_LOOKUP_KIND
        if argv[1] == "llvm-cov":
            # The same distinction, for the same reason. `CARGO_KINDS` maps the
            # tool rather than what it was asked to do, so a version probe or an
            # environment dump used to publish under `coverage` and overwrite a
            # real coverage log with the exit code of a probe.
            #
            # A subcommand this file does not know does *not* land in the lookup
            # kind here, unlike nextest. See `llvm_cov_measures_coverage` for why
            # the safe side is the other one for this tool, and for what makes an
            # unrecognised subcommand fail a check rather than change behaviour.
            if llvm_cov_measures_coverage(argv):
                return CARGO_KINDS["llvm-cov"]
            return LLVM_COV_LOOKUP_KIND
        return CARGO_KINDS.get(argv[1], "cargo")
    return "command"
