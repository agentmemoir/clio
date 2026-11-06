#!/usr/bin/env python3
"""Blocks three Git defects that a macOS checkout and a review cannot repair later.

Each of the three is cheaper to refuse than to work around, and each one arrives silently: nothing in the tree goes red when it lands.

The first is a case collision. The default APFS volume on macOS is case-insensitive, so two tracked paths that differ only in case cannot both exist on disk. A checkout writes one of them and reports success, and the other file is simply absent from that working tree, so a build that depends on it fails on one platform and passes on the other.

The second is an oversized binary. A binary file this change adds or modifies is copied into every clone and into every release archive, and it never compresses away. The size ceiling is withdrawn as a derived limit, on 2026-11-06. The withdrawn value is 512000 bytes, which reads as 500 KiB. No baseline row, no factor, and no measurement ever stood behind it: it was a round KiB number mistaken for a derived ceiling. The value carries the label `Decision`, and its source is this gate's own configuration, not a measurement. Live derived size ceilings for this gate: none. The gate still compares a changed binary against the configured threshold and refuses a binary above it, so its comparison and its product behaviour are unchanged. What is withdrawn is only the claim that the number is derived.

The third is unrelated history. A branch created with `git checkout --orphan`, or a repository grafted onto another, has no common ancestor with master, so a merge of it credits every line to the merge and `git blame` stops answering. `git merge-base` is the question that shows it.

The check is a guard against a mistake, not a proof that the tree is well formed. It reads the index and one diff, and it says what it looked at on every run, including a clean one.

Usage:
  python3 scripts/git_hygiene_check.py --base <rev>
  python3 scripts/git_hygiene_check.py --self-test

--base names the revision the change is compared against: `origin/master` on a runner for a pull request, and `HEAD~1` for a push. Every check reports against it, and the history check names it, so a finding says which comparison produced it.

Exit codes:
  0  no finding: every case-folded path is unique, no changed binary is over the limit, and HEAD shares an ancestor with the base
  1  at least one finding, each printed as a `git-hygiene: FINDING` line
  2  usage error
  3  a git command that had to succeed failed for a reason other than one of the three defects, so the clean parts of the run do not cover it
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path

EXIT_OK = 0
EXIT_FINDINGS = 1
EXIT_USAGE = 2
EXIT_GIT = 3

USAGE = "usage: git_hygiene_check.py --base <rev> | --self-test"

#: The largest binary this change may add or modify, in bytes: an operator-set threshold labelled `Decision`, sourced from this gate's own configuration.
#: The earlier claim that it was a derived ceiling was withdrawn on 2026-11-06: no baseline row, factor, or measurement stood behind it.
#: The module docstring holds the withdrawal record.
LIMIT = 512000

#: The repository this script runs in. The self-test points it at a temporary
#: repository, so every git call goes through an argument rather than this.
ROOT = Path(__file__).resolve().parent.parent


class GitFailure(Exception):
    """A git command that had to succeed failed, so no verdict describes the tree."""


def case_collisions(paths: list[str]) -> list[tuple[str, str]]:
    """Every path that differs from another only by case, as (first, extra).

    The comparison is `str.casefold`, the closest thing to the rule a
    case-insensitive volume applies. The same path seen twice is not a collision
    with itself, so the list is deduplicated first. Groups and the members of a
    group are sorted, so the same index produces the same lines on every host.
    """
    groups: dict[str, list[str]] = {}
    for path in dict.fromkeys(paths):
        groups.setdefault(path.casefold(), []).append(path)
    found: list[tuple[str, str]] = []
    for key in sorted(groups):
        members = sorted(groups[key])
        if len(members) > 1:
            found.extend((members[0], other) for other in members[1:])
    return found


def parse_numstat(text: str) -> list[tuple[str, bool]]:
    """The records of `git diff --numstat --no-renames -z`, as (path, binary).

    With `-z` a record is `added TAB deleted TAB path NUL`, and a binary file
    reports `-` for both counts because it has no lines. `--no-renames` keeps a
    record per path, so a rename is not a single entry whose two paths would
    have to be read out of a different shape.
    """
    records: list[tuple[str, bool]] = []
    for record in text.split("\0"):
        if not record:
            continue
        fields = record.split("\t")
        if len(fields) < 3:
            continue
        added, deleted = fields[0], fields[1]
        records.append(("\t".join(fields[2:]), added == "-" and deleted == "-"))
    return records


def oversized(entries: list[tuple[str, int, bool]]) -> list[tuple[str, int]]:
    """The binary entries larger than LIMIT, as (path, size).

    A text file over the limit is not reported. Its lines are the thing a diff
    and a review can read, and a limit on it would be a rule about something
    else. The size is the one git reports for the blob, so an entry and the
    message agree about which number was compared.
    """
    return [(path, size) for path, size, binary in entries if binary and size > LIMIT]


def evaluate(
    paths: list[str],
    entries: list[tuple[str, int, bool]],
    ancestor: bool,
    base: str,
) -> tuple[int, list[str]]:
    """The verdict over plain data: the exit code and the lines a run prints.

    This is the whole decision, and it takes no repository. The self-test drives
    it directly, so a case-insensitive filesystem is not needed to prove what
    the check decides; the git plumbing only gathers the three inputs.
    """
    collisions = case_collisions(paths)
    big = oversized(entries)
    binaries = sum(1 for _, _, binary in entries if binary)
    findings = [
        f"git-hygiene: FINDING case-collision: {first} and {other}"
        for first, other in collisions
    ]
    findings += [
        f"git-hygiene: FINDING blob-size: {path} is {size} bytes (limit {LIMIT})"
        for path, size in big
    ]
    if not ancestor:
        findings.append(f"git-hygiene: FINDING history: HEAD shares no ancestor with {base}")
    if findings:
        lines = [*findings, f"git-hygiene: {len(findings)} finding(s)"]
        code = EXIT_FINDINGS
    else:
        lines = [
            f"git-hygiene: clean ({len(paths)} tracked file(s), "
            f"{binaries} changed binary file(s))"
        ]
        code = EXIT_OK
    lines.append(
        f"git-hygiene: case-collision: {len(paths)} tracked path(s), "
        f"{len(collisions)} collision(s)"
    )
    lines.append(
        f"git-hygiene: blob-size: {binaries} changed binary file(s), "
        f"{len(big)} over {LIMIT} bytes"
    )
    if ancestor:
        lines.append(
            f"git-hygiene: history: HEAD shares an ancestor with {base}, "
            "so a merge keeps blame"
        )
    else:
        lines.append(f"git-hygiene: history: HEAD and {base} have no common ancestor")
    return code, lines


def _git(root: Path, *args: str) -> subprocess.CompletedProcess:
    """One git command in `root`, with its output captured as bytes."""
    return subprocess.run(["git", *args], cwd=root, capture_output=True, check=False)


def _text(data: bytes) -> str:
    """A git stream as text, keeping a path's own bytes recoverable."""
    return data.decode("utf-8", "surrogateescape")


def _complain(done: subprocess.CompletedProcess, what: str) -> GitFailure:
    """The failure a caller raises, naming the command and git's own message."""
    return GitFailure(f"{what} failed: {_text(done.stderr).strip() or done.returncode}")


def resolve(root: Path, base: str) -> str:
    """The commit `base` names, or GitFailure when it names none."""
    done = _git(root, "rev-parse", "--verify", "--quiet", f"{base}^{{commit}}")
    if done.returncode != 0:
        raise GitFailure(f"the base revision {base!r} does not name a commit")
    return _text(done.stdout).strip()


def tracked_paths(root: Path) -> list[str]:
    """Every path git tracks, one per element."""
    done = _git(root, "ls-files", "-z")
    if done.returncode != 0:
        raise _complain(done, "git ls-files")
    return [path for path in _text(done.stdout).split("\0") if path]


def change_entries(root: Path, commit: str) -> list[tuple[str, int, bool]]:
    """The paths this change adds or modifies, as (path, size, binary).

    A text entry carries a size of 0 because it is never compared against the
    limit; reading every text file's size would be work for an answer nothing
    uses.
    """
    done = _git(
        root, "diff", "--numstat", "--no-renames", "-z", "--diff-filter=AM", commit, "HEAD"
    )
    if done.returncode != 0:
        raise _complain(done, f"git diff {commit} HEAD")
    entries: list[tuple[str, int, bool]] = []
    for path, binary in parse_numstat(_text(done.stdout)):
        size = 0
        if binary:
            sized = _git(root, "cat-file", "-s", f"HEAD:{path}")
            if sized.returncode != 0:
                raise _complain(sized, f"git cat-file -s HEAD:{path}")
            size = int(_text(sized.stdout).strip() or 0)
        entries.append((path, size, binary))
    return entries


def has_ancestor(root: Path, commit: str) -> bool:
    """Whether HEAD and `commit` share a commit, as `git merge-base` answers."""
    return _git(root, "merge-base", commit, "HEAD").returncode == 0


def run(root: Path, base: str) -> tuple[int, list[str]]:
    """The check against a real repository, as (exit code, printed lines)."""
    try:
        commit = resolve(root, base)
        paths = tracked_paths(root)
        entries = change_entries(root, commit)
        ancestor = has_ancestor(root, commit)
    except GitFailure as error:
        return EXIT_GIT, [f"git-hygiene: COULD NOT CHECK {error}"]
    return evaluate(paths, entries, ancestor, base)


def _commit(root: Path, message: str) -> None:
    """Commit the index, with signing off so a host's global setting cannot break it."""
    done = _git(root, "-c", "commit.gpgsign=false", "commit", "-m", message)
    if done.returncode != 0:
        raise SystemExit(f"self-test: could not commit: {_text(done.stderr)}")


def _repository(base: Path) -> tuple[Path, str]:
    """A two-commit repository, and the abbreviation of its first commit."""
    repo = base / "repo"
    repo.mkdir()
    _git(repo, "init", "-b", "main")
    _git(repo, "config", "user.email", "self-test@example.invalid")
    _git(repo, "config", "user.name", "git hygiene self-test")
    (repo / "README.md").write_text("one\n", encoding="utf-8")
    _git(repo, "add", "README.md")
    _commit(repo, "first")
    first = _text(_git(repo, "rev-parse", "HEAD").stdout).strip()
    (repo / "README.md").write_text("one\ntwo\n", encoding="utf-8")
    _git(repo, "add", "README.md")
    _commit(repo, "second")
    return repo, first


def _report(
    name: str,
    code: int,
    lines: list[str],
    want_code: int,
    wanted: list[str],
    failures: int,
    total: int,
) -> tuple[int, int]:
    """Print one case's verdict, counting it and every line it did not print.

    A wanted line is a substring of a printed one, because a case that names a
    commit cannot spell it: the repository the case runs against is created for
    the run, and its hash is not known when the case is written.
    """
    missing = [line for line in wanted if not any(line in actual for actual in lines)]
    ok = code == want_code and not missing
    detail = f" (exit {code}, want {want_code})"
    if missing:
        detail += f" missing {missing}"
    print(f"self-test {'ok' if ok else 'FAIL'}: {name}{detail}")
    return failures + (0 if ok else 1), total + 1


def _self_test() -> int:
    """Drive every rule over plain data, then over a real temporary repository."""
    failures = 0
    total = 0
    cases: list[tuple[str, int, list[str], list[tuple[str, int, bool]], bool, list[str]]] = [
        (
            "no collision",
            EXIT_OK,
            ["README.md", "docs/Using.md", "src/main.rs"],
            [("src/main.rs", 0, False)],
            True,
            [
                "git-hygiene: clean (3 tracked file(s), 0 changed binary file(s))",
                "git-hygiene: case-collision: 3 tracked path(s), 0 collision(s)",
                f"git-hygiene: blob-size: 0 changed binary file(s), 0 over {LIMIT} bytes",
            ],
        ),
        (
            "a collision",
            EXIT_FINDINGS,
            ["README.md", "readme.md"],
            [],
            True,
            [
                "git-hygiene: FINDING case-collision: README.md and readme.md",
                "git-hygiene: 1 finding(s)",
            ],
        ),
        (
            "a three-way collision group",
            EXIT_FINDINGS,
            ["DOC.md", "Doc.md", "doc.md"],
            [],
            True,
            [
                "git-hygiene: FINDING case-collision: DOC.md and Doc.md",
                "git-hygiene: FINDING case-collision: DOC.md and doc.md",
                "git-hygiene: 2 finding(s)",
            ],
        ),
        # A case-only rename is what produces this pair in practice: git records
        # it as one delete and one add, so a case-insensitive checkout holds one
        # file where the index names two.
        (
            "a rename that only changes case",
            EXIT_FINDINGS,
            ["docs/Guide.md", "docs/guide.md"],
            [],
            True,
            ["git-hygiene: FINDING case-collision: docs/Guide.md and docs/guide.md"],
        ),
        (
            "a binary under the limit",
            EXIT_OK,
            ["src/main.rs", "assets/logo.png"],
            [("assets/logo.png", LIMIT, True)],
            True,
            [
                "git-hygiene: clean (2 tracked file(s), 1 changed binary file(s))",
                f"git-hygiene: blob-size: 1 changed binary file(s), 0 over {LIMIT} bytes",
            ],
        ),
        (
            "a binary over the limit",
            EXIT_FINDINGS,
            ["assets/frame.bin"],
            [("assets/frame.bin", LIMIT + 1, True)],
            True,
            [f"git-hygiene: FINDING blob-size: assets/frame.bin is {LIMIT + 1} bytes (limit {LIMIT})"],
        ),
        (
            "a text file over the limit is ignored",
            EXIT_OK,
            ["notes/spec.txt"],
            [("notes/spec.txt", 900000, False)],
            True,
            ["git-hygiene: clean (1 tracked file(s), 0 changed binary file(s))"],
        ),
        (
            "history with no common ancestor",
            EXIT_FINDINGS,
            ["README.md"],
            [],
            False,
            ["git-hygiene: FINDING history: HEAD shares no ancestor with origin/master"],
        ),
    ]
    for name, want_code, paths, entries, ancestor, wanted in cases:
        code, lines = evaluate(paths, entries, ancestor, "origin/master")
        failures, total = _report(
            name, code, lines, want_code, wanted, failures, total
        )

    with tempfile.TemporaryDirectory(prefix="git-hygiene-") as scratch:
        base = Path(scratch)
        repo, first = _repository(base)
        code, lines = run(repo, first)
        failures, total = _report(
            "a real repository with two commits is clean",
            code,
            lines,
            EXIT_OK,
            [
                "git-hygiene: clean (1 tracked file(s), 0 changed binary file(s))",
                "git-hygiene: history: HEAD shares an ancestor with",
            ],
            failures,
            total,
        )

        _git(repo, "checkout", "--orphan", "graft")
        (repo / "graft.txt").write_text("orphan\n", encoding="utf-8")
        _git(repo, "add", "graft.txt")
        _commit(repo, "orphan")
        code, lines = run(repo, first)
        failures, total = _report(
            "a real repository with no common ancestor is a finding",
            code,
            lines,
            EXIT_FINDINGS,
            [f"git-hygiene: FINDING history: HEAD shares no ancestor with {first}"],
            failures,
            total,
        )

    if failures:
        print(f"self-test: {failures} of {total} failed")
        return EXIT_FINDINGS
    print(f"self-test: all passed ({total} cases)")
    return EXIT_OK


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return _self_test()
    if "-h" in argv or "--help" in argv:
        print(__doc__.strip())
        return EXIT_OK
    if len(argv) != 2 or argv[0] != "--base" or not argv[1].strip():
        print(USAGE, file=sys.stderr)
        return EXIT_USAGE
    code, lines = run(ROOT, argv[1])
    stream = sys.stderr if code == EXIT_GIT else sys.stdout
    for line in lines:
        print(line, file=stream)
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
