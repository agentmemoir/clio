"""The changed files Quick cannot pick a Rust test for, and what to do about each.

`scoped_runner.changed_files` keeps only `.rs` paths, so a change to `Cargo.lock`, a
`.sql` file a crate embeds, or a deleted Rust file selected nothing, and Quick said
"no changed Rust file" and exited 0. A reader took that for a pass. This module takes the
other changed paths from `scoped_runner.changed_paths` so Quick can widen to the package
that owns one, or say plainly that it tested nothing for it. The scoped runner treats the
same files as build inputs (`scoped_select.widens_to_every_package`) and runs every package a lock
file reaches; Quick keeps to the 60 s budget and lists those instead.

    in a package folder        -> the whole package (the file can change what its tests read)
    under sql/                 -> every package whose source names the file
    deleted Rust file          -> listed: its dependents are checked only by CI
    anything else, not `.md`   -> listed: Quick selects no test for it
"""

from __future__ import annotations

from typing import NamedTuple, Sequence

import scoped_runner as runner
from quick_terms import Term

SHOWN = 5


class Untested(NamedTuple):
    terms: list[Term]
    notes: list[str]
    left_out: list[str]


def other_changed(base: str) -> list[str]:
    """Changed paths that are neither Rust source nor Markdown."""
    return [path for path in runner.changed_paths(base) if not path.endswith((".rs", ".md"))]


def classify(base: str, packages: Sequence[dict], deleted: Sequence[str]) -> Untested:
    terms: list[Term] = []
    notes: list[str] = []
    left_out = [f"{rel} (deleted; only CI checks what depended on it)" for rel in deleted]
    for rel in other_changed(base):
        owners = runner.package_owners(rel, packages) or (sorted(runner.embedders(rel, packages)) if rel.startswith("sql/") else [])
        if not owners:
            left_out.append(rel)
            continue
        for package in owners:
            terms.append(Term(package, reason=f"{rel} is not Rust source"))
            notes.append(f"{rel} -> {package}: whole package, it is not Rust source")
    return Untested(terms, notes, left_out)


def left_out_lines(left_out: Sequence[str]) -> list[str]:
    """The lines that say what this run selected no test for."""
    if not left_out:
        return []
    shown = ", ".join(left_out[:SHOWN]) + (f", and {len(left_out) - SHOWN} more" if len(left_out) > SHOWN else "")
    return [f"quick: no test selected for {len(left_out)} changed file(s): {shown}", "quick: CI decides those"]
