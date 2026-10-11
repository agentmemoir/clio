"""Fixture texts and the fixture builder for the lint-scope self-test."""

from __future__ import annotations

from pathlib import Path

# --- Self-test -----------------------------------------------------------------
#
# Every case is a fixture tree, so the cases describe the check rather than the
# repository and the suite gives the same answer on any machine. The fixtures live
# under one temporary directory that is removed when the suite returns, because a
# case per directory left sixteen trees behind on every pre-commit and every CI
# run. Nothing here is executed, so the executable-temporary-directory helper the
# other suites need is not needed: no fixture has to run.

CLEAN_MAKEFILE = """PACKAGES ?= --workspace

ritual:
\tcargo fmt --all -- --check
\tcargo clippy --workspace --all-targets --all-features --locked -- -D warnings

lint:
\tcargo clippy $(PACKAGES) --all-targets --all-features --locked -- -D warnings
"""

CLEAN_WORKFLOW = """on: [push]
jobs:
  check:
    steps:
      - uses: dtolnay/rust-toolchain@master
        with:
          toolchain: 1.98.1
      - name: Clippy
        run: cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
"""

CLEAN_MANIFEST = '[workspace]\nmembers = ["a", "b"]\n\n[workspace.package]\nversion = "0.1.0"\n'

CLEAN_FILES = {
    "Makefile": CLEAN_MAKEFILE,
    "Cargo.toml": CLEAN_MANIFEST,
    ".github/workflows/ci.yml": CLEAN_WORKFLOW,
}

#: One bare clippy line, used by the cases that vary something else.
BARE = "ritual:\n\tcargo clippy --all-targets --all-features --locked -- -D warnings\n"

BARE_IN_WORKFLOW = """on: [push]
jobs:
  check:
    steps:
      - name: Clippy
        run: cargo clippy --all-targets --all-features --locked -- -D warnings
"""

#: A toolchain named on the command line in front of a bare `cargo`, and the same
#: run with no name at all. The clippy arguments are spelled out in each fixture
#: rather than produced by editing `CLEAN_MAKEFILE`, because substring surgery on
#: a fixture is how a case ends up testing a line it never built.
RUSTUP_RUN_MAKEFILE = (
    "ritual:\n\trustup run stable cargo clippy --workspace --all-targets"
    " --all-features --locked -- -D warnings\n"
)

RUSTUP_RUN_BARE_MAKEFILE = (
    "ritual:\n\trustup run cargo clippy --workspace --all-targets"
    " --all-features --locked -- -D warnings\n"
)

RUSTUP_TOOLCHAIN_MAKEFILE = (
    "ritual:\n\tRUSTUP_TOOLCHAIN=stable cargo clippy --workspace --all-targets"
    " --all-features --locked -- -D warnings\n"
)

RUSTC_ENV_MAKEFILE = (
    "ritual:\n\tRUSTC=/some/path/rustc cargo clippy --workspace --all-targets"
    " --all-features --locked -- -D warnings\n"
)

#: The override sits *after* the executable, so nothing in front of the cargo
#: token carries it and the prefix rules cannot see it at all.
CARGO_PLUS_TC_MAKEFILE = (
    "ritual:\n\tcargo +stable clippy --workspace --all-targets --all-features"
    " --locked -- -D warnings\n"
)

#: One bare line and one overridden line, so the case cannot pass on the count
#: alone: the finding has to name the `+stable` line specifically.
CARGO_PLUS_TC_MIXED_MAKEFILE = (
    "lint:\n\tcargo clippy --workspace --all-targets --locked -- -D warnings\n"
    "\nritual:\n\tcargo +nightly clippy --workspace --all-targets --all-features"
    " --locked -- -D warnings\n"
)

#: `+` with no name is not a toolchain, so refusing it would refuse a spelling
#: that overrides nothing.
CARGO_PLUS_BARE_MAKEFILE = (
    "ritual:\n\tcargo + clippy --workspace --all-targets --all-features"
    " --locked -- -D warnings\n"
)

#: Two clippy lines, so the brace case cannot pass on there being at least one
#: invocation: the bare line is what keeps the run count up, and the `${CARGO}`
#: line is the one that has to be found and refused.
BRACE_CARGO_MAKEFILE = (
    "lint:\n\tcargo clippy --workspace --all-targets --locked -- -D warnings\n"
    "\nritual:\n\t${CARGO} clippy --workspace --all-targets --all-features"
    " --locked -- -D warnings\n"
)

CARGO_BARE_MAKEFILE = (
    "CARGO ?= cargo\n\nritual:\n\t$(CARGO) clippy --workspace --all-targets"
    " --all-features --locked -- -D warnings\n"
)

#: The same fallback written the other way round, as an export.
CARGO_EXPORT_MAKEFILE = (
    "export CARGO = cargo\n\nritual:\n\t$(CARGO) clippy --workspace"
    " --all-targets --locked -- -D warnings\n"
)

CARGO_TWICE_MAKEFILE = (
    "CARGO ?= cargo\nCARGO := /usr/bin/cargo\n\nritual:\n"
    "\t$(CARGO) clippy --workspace --all-targets --all-features --locked"
    " -- -D warnings\n"
)


def _fixture(base: Path, label: str, changes: dict[str, str | None] | None = None) -> Path:
    """One case's tree: the clean files, with `changes` applied over them.

    The overrides are a dict of real relative paths rather than keyword arguments,
    because a keyword cannot carry the dot in `Cargo.toml` and a path mangling
    scheme cannot carry the slashes. Getting that wrong wrote the override to
    `Cargo/toml`, left the clean manifest in place, and two cases passed because
    they were testing the wrong file.
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
