"""The lint-scope self-test: each case builds a tree and states what the scan must say."""

from __future__ import annotations

import contextlib
import io
import tempfile
from pathlib import Path

from lint_scope_core import EXIT_FINDINGS, EXIT_OK, EXIT_UNREADABLE, report, scan
from lint_scope_fixtures import (
    BARE,
    BARE_IN_WORKFLOW,
    BRACE_CARGO_MAKEFILE,
    CARGO_BARE_MAKEFILE,
    CARGO_EXPORT_MAKEFILE,
    CARGO_PLUS_BARE_MAKEFILE,
    CARGO_PLUS_TC_MAKEFILE,
    CARGO_PLUS_TC_MIXED_MAKEFILE,
    CARGO_TWICE_MAKEFILE,
    CLEAN_MAKEFILE,
    CLEAN_WORKFLOW,
    RUSTC_ENV_MAKEFILE,
    RUSTUP_RUN_BARE_MAKEFILE,
    RUSTUP_RUN_MAKEFILE,
    RUSTUP_TOOLCHAIN_MAKEFILE,
    _fixture,
)
from lint_scope_text import clippy_invocations


def _cases(base: Path) -> list[tuple[str, int, Path]]:
    """(name, expected exit, fixture root) for every case needing no refusal."""
    wf = ".github/workflows/ci.yml"
    return [
        ("a clean tree passes", EXIT_OK, _fixture(base, "clean")),
        ("a bare clippy in the Makefile is a finding", EXIT_FINDINGS, _fixture(base, "makefile-bare", {"Makefile": BARE})),
        ("a bare clippy in a workflow is a finding", EXIT_FINDINGS, _fixture(base, "workflow-bare", {wf: BARE_IN_WORKFLOW})),
        ("a bare clippy in a shell script is a finding", EXIT_FINDINGS, _fixture(base, "script-bare", {"scripts/lint.sh": "#!/bin/sh\ncargo clippy --all-targets -- -D warnings\n"})),
        (
            "a shell script holding a scoped clippy is clean, at the same path",
            EXIT_OK,
            _fixture(base, "script-clean", {"scripts/lint.sh": "#!/bin/sh\ncargo clippy --workspace --all-targets -- -D warnings\n"}),
        ),
        (
            "the shim carrying a dispatch arm is read and stays clean",
            EXIT_OK,
            _fixture(base, "shim", {"scripts/gate-shim": 'case "$1" in\n  test | nextest | llvm-cov | clippy) heavy=1 ;;\nesac\n'}),
        ),
        ("--all-targets alone is not a package selector", EXIT_FINDINGS, _fixture(base, "all-targets", {"Makefile": "ritual:\n\tcargo clippy --all-targets --locked -- -D warnings\n"})),
        (
            "--workspace --exclude drops a crate and is a finding",
            EXIT_FINDINGS,
            _fixture(base, "exclude-makefile", {"Makefile": "ritual:\n\tcargo clippy --workspace --exclude clio-store --all-targets --locked -- -D warnings\n"}),
        ),
        (
            "--workspace --exclude in a workflow is a finding",
            EXIT_FINDINGS,
            _fixture(base, "exclude-workflow", {wf: CLEAN_WORKFLOW.replace("--workspace --all-targets", "--workspace --exclude clio-store --all-targets")}),
        ),
        # The executable, which is a different defect from the scope of the run.
        # The `$(CARGO)` form is the line this rule was added for, written out
        # rather than produced by editing the clean fixture, because substring
        # surgery on a fixture is how a case ends up testing a line it never built.
        (
            "$(CARGO) clippy is a finding",
            EXIT_FINDINGS,
            _fixture(base, "cargo-var", {"Makefile": "PACKAGES ?= --workspace\n\nritual:\n\t$(CARGO) clippy --workspace --all-targets --all-features --locked -- -D warnings\n"}),
        ),
        (
            "$(RUSTUP_BIN)/cargo clippy is a finding",
            EXIT_FINDINGS,
            _fixture(base, "rustup-bin", {"Makefile": "ritual:\n\t$(RUSTUP_BIN)/cargo clippy --workspace --all-targets --locked -- -D warnings\n"}),
        ),
        (
            "a clippy run through an absolute path is a finding",
            EXIT_FINDINGS,
            _fixture(base, "abs-path", {"Makefile": "ritual:\n\t/usr/local/rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo clippy --workspace --all-targets --locked -- -D warnings\n"}),
        ),
        (
            "a bare cargo clippy in the Makefile is clean",
            EXIT_OK,
            _fixture(base, "makefile-shim", {"Makefile": "PACKAGES ?= --workspace\n\nritual:\n\tcargo clippy --workspace --all-targets --locked -- -D warnings\n"}),
        ),
        (
            "$(CARGO) on a line that runs no clippy is clean, and the workflow's bare cargo with it",
            EXIT_OK,
            _fixture(base, "cargo-var-no-clippy", {"Makefile": "CARGO ?= /usr/bin/cargo\nPACKAGES ?= --workspace\n\nbuild:\n\t$(CARGO) build --locked\n\nritual:\n\tcargo clippy --workspace --all-targets --locked -- -D warnings\n"}),
        ),
        (
            "a trailing comment naming the selector does not supply it",
            EXIT_FINDINGS,
            _fixture(base, "todo-comment", {"Makefile": BARE.rstrip("\n") + " # TODO: add --workspace\n"}),
        ),
        (
            "a trailing comment in a workflow does not supply the selector",
            EXIT_FINDINGS,
            _fixture(base, "todo-workflow", {wf: BARE_IN_WORKFLOW.rstrip("\n") + "\n          # same as --workspace\n"}),
        ),
        (
            "a second PACKAGES definition that narrows is a finding",
            EXIT_FINDINGS,
            _fixture(base, "packages-twice", {"Makefile": CLEAN_MAKEFILE.replace("PACKAGES ?= --workspace", "PACKAGES ?= --workspace\nPACKAGES := -p clio")}),
        ),
        (
            "a wrapped recipe is read as one command",
            EXIT_OK,
            _fixture(base, "wrapped", {"Makefile": "PACKAGES ?= --workspace\n\nritual:\n\tcargo clippy \\\n\t\t--workspace --all-targets --locked -- -D warnings\n"}),
        ),
        (
            "a selector in any variable, not only PACKAGES, is honoured",
            EXIT_OK,
            _fixture(base, "other-variable", {"Makefile": "SCOPE := --workspace\n\nritual:\n\tcargo clippy $(SCOPE) --all-targets --locked -- -D warnings\n"}),
        ),
        (
            "a variable that does not name the workspace is a finding",
            EXIT_FINDINGS,
            _fixture(base, "other-variable-narrow", {"Makefile": "SCOPE := -p clio\n\nritual:\n\tcargo clippy $(SCOPE) --all-targets --locked -- -D warnings\n"}),
        ),
        (
            "-p names packages and passes",
            EXIT_OK,
            _fixture(base, "dash-p", {"Makefile": "ritual:\n\tcargo clippy -p a -p b --all-targets --locked -- -D warnings\n"}),
        ),
        ("$(PACKAGES) is accepted while it still means the workspace", EXIT_OK, _fixture(base, "packages-whole")),
        (
            "$(PACKAGES) narrowed to one crate is a finding",
            EXIT_FINDINGS,
            _fixture(base, "packages-narrow", {"Makefile": CLEAN_MAKEFILE.replace("PACKAGES ?= --workspace", "PACKAGES ?= -p clio")}),
        ),
        (
            "default-members is a finding",
            EXIT_FINDINGS,
            _fixture(base, "default-members", {"Cargo.toml": '[workspace]\nmembers = ["a"]\ndefault-members = ["a"]\n'}),
        ),
        (
            "a quoted default-members key is the same key",
            EXIT_FINDINGS,
            _fixture(base, "default-members-quoted", {"Cargo.toml": '[workspace]\nmembers = ["a"]\n"default-members" = ["a"]\n'}),
        ),
        (
            "default-members outside [workspace] is not a finding",
            EXIT_OK,
            _fixture(base, "default-members-elsewhere", {"Cargo.toml": '[workspace]\nmembers = ["a"]\n\n[profile.dev]\ndefault-members = ["a"]\n'}),
        ),
        ("a deprecated --all is reported and still names the scope", EXIT_FINDINGS, _fixture(base, "all-alias", {"Makefile": CLEAN_MAKEFILE.replace("clippy --workspace", "clippy --all")})),
        ("a dispatch arm naming clippy is not an invocation", EXIT_OK, _fixture(base, "dispatch-arm", {"hooks.d/x.sh": 'case "$1" in\n  test | clippy) heavy=1 ;;\nesac\n'})),
        ("a commented clippy line is not an invocation", EXIT_OK, _fixture(base, "commented", {"Makefile": CLEAN_MAKEFILE + "# $(CARGO) clippy --all-targets -- -D warnings\n"})),
        ("a missing Makefile is a finding, not a pass", EXIT_FINDINGS, _fixture(base, "no-makefile", {"Makefile": None})),
        (
            "a Makefile with no clippy run is a finding",
            EXIT_FINDINGS,
            _fixture(base, "no-clippy", {"Makefile": "PACKAGES ?= --workspace\n\nritual:\n\tcargo fmt --all -- --check\n", wf: "on: [push]\njobs: {}\n"}),
        ),
        (
            "a workflow that mentions clippy but runs none is a finding",
            EXIT_FINDINGS,
            _fixture(base, "workflow-lost", {wf: "on: [push]\njobs:\n  check:\n    steps:\n      - name: Clippy would go here\n"}),
        ),
        # A toolchain named around a bare `cargo`, which the executable check
        # cannot see because the executable really is the bare shim. Each of
        # these was accepted with exit 0 before.
        (
            "rustup run naming a toolchain before a bare cargo is a finding",
            EXIT_FINDINGS,
            _fixture(base, "rustup-run", {"Makefile": RUSTUP_RUN_MAKEFILE}),
        ),
        (
            "rustup run naming no toolchain before a bare cargo is clean",
            EXIT_OK,
            _fixture(base, "rustup-run-bare", {"Makefile": RUSTUP_RUN_BARE_MAKEFILE}),
        ),
        (
            "RUSTUP_TOOLCHAIN on the clippy line is a finding",
            EXIT_FINDINGS,
            _fixture(base, "rustup-toolchain-env", {"Makefile": RUSTUP_TOOLCHAIN_MAKEFILE}),
        ),
        (
            "cargo +toolchain before the subcommand is a finding",
            EXIT_FINDINGS,
            _fixture(base, "cargo-plus-tc", {"Makefile": CARGO_PLUS_TC_MAKEFILE}),
        ),
        (
            "cargo +toolchain is refused while a bare cargo on another line is clean",
            EXIT_FINDINGS,
            _fixture(base, "cargo-plus-tc-mixed", {"Makefile": CARGO_PLUS_TC_MIXED_MAKEFILE}),
        ),
        (
            "a bare + with no toolchain name is not a toolchain override",
            EXIT_OK,
            _fixture(base, "cargo-plus-bare", {"Makefile": CARGO_PLUS_BARE_MAKEFILE}),
        ),
        (
            "RUSTC on the clippy line is a finding",
            EXIT_FINDINGS,
            _fixture(base, "rustc-env", {"Makefile": RUSTC_ENV_MAKEFILE}),
        ),
        (
            "a second CARGO definition carrying a path is a finding",
            EXIT_FINDINGS,
            _fixture(base, "cargo-path-twice", {"Makefile": CARGO_TWICE_MAKEFILE}),
        ),
        # The executable, resolved through the file's own definitions.
        (
            "a ${CARGO} clippy line is a finding",
            EXIT_FINDINGS,
            _fixture(base, "brace-cargo", {"Makefile": BRACE_CARGO_MAKEFILE}),
        ),
        (
            "$(CARGO) holding a bare cargo is clean",
            EXIT_OK,
            _fixture(base, "cargo-bare", {"Makefile": CARGO_BARE_MAKEFILE}),
        ),
        (
            "an exported CARGO holding a bare cargo is clean",
            EXIT_OK,
            _fixture(base, "cargo-export", {"Makefile": CARGO_EXPORT_MAKEFILE}),
        ),
    ]


def _refusal_case(base: Path) -> Path | None:
    """A fixture whose only problem is a file this process cannot read.

    None when the refusal cannot be provoked here. A host that reads a mode-000
    file anyway reports the skip, because a case that cannot be set up is not a
    case that ran.
    """
    root = _fixture(base, "unreadable", {"scripts/lint.sh": "#!/bin/sh\ncargo clippy --workspace\n"})
    blocked = root / "scripts" / "lint.sh"
    blocked.chmod(0o000)
    try:
        blocked.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return root
    return None


def _self_test() -> int:
    failures = 0
    total = 0
    skipped = 0
    with tempfile.TemporaryDirectory(prefix="lint-scope-") as scratch:
        base = Path(scratch)
        for name, want, root in _cases(base):
            total += 1
            got = scan(root).exit_code()
            ok = got == want
            print(f"self-test {'ok' if ok else 'FAIL'}: {name} (exit {got}, want {want})")
            failures += 0 if ok else 1

        # The cases above read an exit code, and an exit code cannot say which
        # rule fired or on which line. A fixture whose Makefile was never read
        # raises the same code as one whose clippy line was refused, which is
        # exactly how a case passes for the wrong reason. So this asserts the
        # finding itself, against a whole fixture tree: the Makefile carries the
        # executable by reference, the workflow carries a bare one, and the rule
        # has to name the first and leave the second alone.
        total += 1
        mixed = _fixture(
            base,
            "executable-content",
            {
                "Makefile": "PACKAGES ?= --workspace\n\nritual:\n"
                "\t$(CARGO) clippy --workspace --all-targets --locked -- -D warnings\n"
            },
        )
        surface = scan(mixed)
        # Matched on the reason the message states, not on a rule label, because
        # the label is the check's business and the reason is the finding's.
        refused = [f for f in surface.findings if "rust-toolchain.toml" in f.message]
        want_line = next(
            number
            for number, raw in enumerate((mixed / "Makefile").read_text().splitlines(), start=1)
            if "$(CARGO) clippy" in raw
        )
        got_where = [(f.path, f.line) for f in refused]
        ok = (
            surface.exit_code() == EXIT_FINDINGS
            and got_where == [("Makefile", want_line)]
            and "`$(CARGO)`" in refused[0].message
        )
        print(
            f"self-test {'ok' if ok else 'FAIL'}: the finding names the Makefile line that "
            f"reaches cargo through $(CARGO) and not the workflow's bare cargo "
            f"(reported {got_where}, want [('Makefile', {want_line})])"
        )
        failures += 0 if ok else 1

        # An exit code cannot say which rule fired, on which line, or what the
        # check printed. The three assertions below read the findings and the
        # output, so a case that passed because a different rule fired, or
        # because the fixture's own Makefile line was never read, cannot pass.
        total += 1
        brace = _fixture(base, "brace-counted", {"Makefile": BRACE_CARGO_MAKEFILE})
        brace_surface = scan(brace)
        brace_lines = clippy_invocations((brace / "Makefile").read_text(encoding="utf-8"))
        brace_findings = [f for f in brace_surface.findings if f.path == "Makefile"]
        ok = (
            len(brace_lines) == 2
            and len(brace_findings) == 1
            and brace_findings[0].line == 5
            and "${CARGO}" in brace_findings[0].message
        )
        print(
            f"self-test {'ok' if ok else 'FAIL'}: both clippy lines are counted and only the "
            f"${{CARGO}} one is refused (counted {len(brace_lines)}, want 2; findings on lines "
            f"{[f.line for f in brace_findings]}, want [5])"
        )
        failures += 0 if ok else 1

        total += 1
        override = _fixture(base, "override-printed", {"Makefile": RUSTUP_TOOLCHAIN_MAKEFILE})
        printed = io.StringIO()
        with contextlib.redirect_stdout(printed):
            got = report(override)
        output = printed.getvalue()
        ok = (
            got == EXIT_FINDINGS
            and "lint-scope-check: FINDING Makefile:2: scope:" in output
            and "`RUSTUP_TOOLCHAIN=stable` is an override rustup resolves ahead of the "
            "toolchain file" in output
        )
        print(
            f"self-test {'ok' if ok else 'FAIL'}: the whole check reports the override line with "
            f"its file, line, rule and reason (exit {got}, want {EXIT_FINDINGS})"
        )
        failures += 0 if ok else 1

        total += 1
        bare = _fixture(base, "bare-definitions", {"Makefile": CARGO_BARE_MAKEFILE})
        bare_surface = scan(bare)
        ok = (
            bare_surface.exit_code() == EXIT_OK
            and not [f for f in bare_surface.findings if f.path == "Makefile"]
        )
        print(
            f"self-test {'ok' if ok else 'FAIL'}: $(CARGO) holding a bare cargo is clean under the "
            f"whole check (exit {bare_surface.exit_code()}, want {EXIT_OK}, "
            f"findings {[str(f) for f in bare_surface.findings]})"
        )
        failures += 0 if ok else 1

        root = _refusal_case(base)
        if root is None:
            total += 1
            skipped += 1
            print("self-test skipped: this process reads a mode-000 file, so the refusal cannot be provoked here")
        else:
            total += 1
            got = scan(root).exit_code()
            ok = got == EXIT_UNREADABLE
            print(
                f"self-test {'ok' if ok else 'FAIL'}: a clippy script this process cannot read "
                f"fails closed (exit {got}, want {EXIT_UNREADABLE})"
            )
            failures += 0 if ok else 1
            (root / "scripts" / "lint.sh").chmod(0o600)

    if failures:
        print(f"self-test: {failures} of {total} failed")
        return EXIT_FINDINGS
    print(f"self-test: all passed ({total - skipped} cases, {skipped} skipped)")
    return EXIT_OK
