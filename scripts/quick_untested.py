"""The changed files Quick cannot pick a Rust test for, and what to do about each.

`scoped_runner.changed_files` keeps only `.rs` paths, so a change to `Cargo.lock`, a
`.sql` file a crate embeds, or a deleted Rust file selected nothing, and Quick said
"no changed Rust file" and exited 0. A reader took that for a pass. This module finds
those files so Quick can widen to the package that owns one, or say plainly that it
tested nothing for it.

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
    """Changed paths that are neither Rust source nor Markdown: the same four git queries as
    `scoped_runner.changed_files`, without its `.rs` filter."""
    paths: set[str] = set()
    sha = runner.merge_base(base)
    if sha is not None:
        paths.update(runner.git("diff", "--name-only", f"{sha}..HEAD").splitlines())
    paths.update(runner.git("diff", "--name-only", "--cached").splitlines())
    paths.update(runner.git("diff", "--name-only").splitlines())
    paths.update(runner.git("ls-files", "--others", "--exclude-standard", "-z").split("\0"))
    return sorted(p for p in paths if p and not p.endswith((".rs", ".md")))


def sql_owners(rel: str, packages: Sequence[dict]) -> list[str]:
    """Packages whose Rust source names `rel`, which they embed with `include_str!`."""
    try:
        found = runner.git("grep", "-l", "-F", rel, "--", "*.rs").splitlines()
    except runner.RunError:  # git grep exits 1 when nothing matches
        return []
    return sorted({owner for path in found for owner in runner.package_owners(path, packages)})


def classify(base: str, packages: Sequence[dict], deleted: Sequence[str]) -> Untested:
    terms: list[Term] = []
    notes: list[str] = []
    left_out = [f"{rel} (deleted; only CI checks what depended on it)" for rel in deleted]
    for rel in other_changed(base):
        owners = runner.package_owners(rel, packages) or (sql_owners(rel, packages) if rel.startswith("sql/") else [])
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
