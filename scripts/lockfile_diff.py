#!/usr/bin/env python3
"""Summarise the crates a Cargo.lock change adds, removes, and moves between versions.

A textual lockfile diff hides what actually moved: the file is sorted, every line carries a checksum, and a single dependency bump rewrites the blocks around it, so a reviewer reads a page of `version` and `checksum` lines to find that one crate went from 1.0.0 to 1.0.1. This script reads two revisions of the lockfile and prints only the crate-level answer, as markdown for `$GITHUB_STEP_SUMMARY`.

Both revisions are read with `git show <rev>:<path>` rather than from the working tree, so the summary describes the two commits rather than whatever is checked out. Each is parsed with `tomllib`, and only the `name` and `version` of every `[[package]]` entry is kept.

A crate is reported in exactly one section. A name the base revision does not have is Added, one the head revision does not have is Removed, and a name both have with a different set of versions is Updated, with the old versions and the new ones comma-joined and sorted. A crate that gains a second version therefore reads as one Updated row rather than as one Added row beside a stale one, and the three counts add up to the number of crates that moved.

Usage:
  python3 scripts/lockfile_diff.py --base <rev> [--head <rev>] [--path Cargo.lock] [--output FILE]
  python3 scripts/lockfile_diff.py --self-test

--head defaults to HEAD and --path to Cargo.lock. The markdown goes to --output when it is given and to stdout otherwise; the one-line `lockfile-diff: N added, M removed, K updated` status is printed last, on stderr when a file was written and on stdout when the markdown went there.

Exit codes:
  0  a summary was produced, whether or not the lockfile changed
  2  usage error
  3  a revision or the lockfile could not be read, so no summary describes it
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

EXIT_OK = 0
EXIT_USAGE = 2
EXIT_UNREADABLE = 3

#: The repository this script runs in. The self-test points it at a temporary
#: repository, so every git call goes through it.
ROOT = Path(__file__).resolve().parent.parent

DEFAULT_PATH = "Cargo.lock"
DEFAULT_HEAD = "HEAD"

#: Rows per table. A lockfile change of several hundred crates is a regeneration,
#: and the step summary is a place to start reading rather than the whole list.
MAX_ROWS = 50

SHORT = 7

USAGE = "usage: lockfile_diff.py --base <rev> [--head <rev>] [--path Cargo.lock] [--output FILE] | --self-test"


class Unreadable(Exception):
    """A revision, or the lockfile in it, could not be read."""


def git(*args: str) -> str:
    """The stdout of one git command run in the repository, or Unreadable."""
    try:
        proc = subprocess.run(["git", *args], capture_output=True, text=True, check=False, cwd=ROOT)
    except OSError as error:
        raise Unreadable(f"git could not run ({error})") from error
    if proc.returncode != 0:
        detail = proc.stderr.strip() or proc.stdout.strip() or f"exit {proc.returncode}"
        raise Unreadable(detail)
    return proc.stdout


def short_rev(rev: str) -> str:
    """The abbreviation the summary prints for a revision."""
    # `--short=<n>` with an `=`, because the space-separated form is read as a
    # revision of its own and fails with "Needed a single revision".
    return git("rev-parse", f"--short={SHORT}", rev).strip()


def lockfile_text(rev: str, path: str) -> str:
    """One revision's lockfile, as git stored it."""
    return git("show", f"{rev}:{path}")


def versions(text: str) -> dict[str, set[str]]:
    """Crate name to the versions the `[[package]]` entries give it."""
    try:
        data = tomllib.loads(text)
    except tomllib.TOMLDecodeError as error:
        raise Unreadable(f"the lockfile is not valid TOML ({error})") from error
    packages = data.get("package", [])
    if not isinstance(packages, list):
        raise Unreadable("`package` is not an array of tables, so the lockfile has no crates to compare")
    found: dict[str, set[str]] = {}
    for package in packages:
        if not isinstance(package, dict):
            continue
        name = package.get("name")
        version = package.get("version")
        if isinstance(name, str) and isinstance(version, str):
            found.setdefault(name, set()).add(version)
    return found


def compare(
    base: dict[str, set[str]],
    head: dict[str, set[str]],
) -> tuple[list[list[str]], list[list[str]], list[list[str]]]:
    """The added, removed, and updated crates, each sorted, as table rows."""
    added = [[name, version] for name in sorted(head) if name not in base for version in sorted(head[name])]
    removed = [[name, version] for name in sorted(base) if name not in head for version in sorted(base[name])]
    updated = [
        [name, ", ".join(sorted(base[name])), ", ".join(sorted(head[name]))]
        for name in sorted(base)
        if name in head and base[name] != head[name]
    ]
    return added, removed, updated


def table(title: str, header: list[str], rows: list[list[str]]) -> list[str]:
    """One markdown table, capped, with the number of omitted rows stated."""
    lines = ["", f"### {title}", "", "| " + " | ".join(header) + " |", "| " + " | ".join("---" for _ in header) + " |"]
    lines += ["| " + " | ".join(row) + " |" for row in rows[:MAX_ROWS]]
    if len(rows) > MAX_ROWS:
        lines += ["", f"_and {len(rows) - MAX_ROWS} more_"]
    return lines


def render(base_rev: str, head_rev: str, base: dict[str, set[str]], head: dict[str, set[str]]) -> tuple[str, str]:
    """The markdown summary and its status line, for two parsed lockfiles."""
    added, removed, updated = compare(base, head)
    status = f"{len(added)} added, {len(removed)} removed, {len(updated)} updated"
    lines = ["## Cargo.lock diff", "", f"`{base_rev}` to `{head_rev}`: {status}."]
    if not (added or removed or updated):
        lines += ["", "No crate additions, removals, or version changes."]
        return "\n".join(lines) + "\n", status
    lines += table("Added", ["crate", "version"], added)
    lines += table("Removed", ["crate", "version"], removed)
    lines += table("Updated", ["crate", "from", "to"], updated)
    return "\n".join(lines) + "\n", status


def summary(base: str, head: str, path: str) -> tuple[str, str]:
    """The markdown and its status line for two revisions, or Unreadable."""
    base_short = short_rev(base)
    head_short = short_rev(head)
    return render(base_short, head_short, versions(lockfile_text(base, path)), versions(lockfile_text(head, path)))


def write(stream, text: str) -> None:
    """One write to a stream, so a long table is not interleaved with anything."""
    stream.write(text)


def self_test() -> int:
    """The comparison, the rendering, and both exit codes, against fixtures."""
    failed = 0

    def expect(label: str, got, want) -> None:
        nonlocal failed
        if got != want:
            failed += 1
            print(f"lockfile-diff self-test: {label}: expected {want!r}, got {got!r}")

    def lockfile(*packages: tuple[str, str]) -> str:
        body = "".join(
            f'\n[[package]]\nname = "{name}"\nversion = "{version}"\n' for name, version in packages
        )
        return f"version = 3\n{body}"

    one = lockfile(("clio", "0.1.0"), ("serde", "1.0.0"))
    expect("one crate per name", versions(one), {"clio": {"0.1.0"}, "serde": {"1.0.0"}})
    expect("two versions of one name", versions(lockfile(("syn", "1.0.0"), ("syn", "2.0.0"))), {"syn": {"1.0.0", "2.0.0"}})
    expect("no package table", versions("version = 3\n"), {})

    # No change: a summary is still produced, and it prints no table.
    text, status = render("aaaaaaa", "bbbbbbb", versions(one), versions(one))
    expect("no change status", status, "0 added, 0 removed, 0 updated")
    expect("no change sentence", "No crate additions, removals, or version changes." in text, True)
    expect("no change has no tables", "### Added" in text or "| crate |" in text, False)
    expect("no change names both revisions", "`aaaaaaa` to `bbbbbbb`: 0 added, 0 removed, 0 updated." in text, True)

    # One crate added, and nothing else.
    text, status = render("aaaaaaa", "bbbbbbb", versions(one), versions(one + "\n[[package]]\nname = \"tokio\"\nversion = \"1.0.0\"\n"))
    expect("added status", status, "1 added, 0 removed, 0 updated")
    expect("added row", "| tokio | 1.0.0 |" in text, True)
    expect("added section", "### Added" in text, True)
    expect("added keeps removed empty", "### Removed" in text and "| clio |" not in text.split("### Removed", 1)[1].split("###", 1)[0], True)

    # One crate removed.
    text, status = render("aaaaaaa", "bbbbbbb", versions(one), versions(lockfile(("clio", "0.1.0"))))
    expect("removed status", status, "0 added, 1 removed, 0 updated")
    expect("removed row", "| serde | 1.0.0 |" in text, True)

    # One version update: one row, in Updated, and in neither of the other two.
    text, status = render("aaaaaaa", "bbbbbbb", versions(one), versions(lockfile(("clio", "0.1.0"), ("serde", "1.0.1"))))
    expect("update status", status, "0 added, 0 removed, 1 updated")
    expect("update row", "| serde | 1.0.0 | 1.0.1 |" in text, True)
    expect("update is not also an addition", "### Added" in text and "| serde | 1.0.1 |" not in text, True)

    # A name with two versions in one revision: the versions are joined and sorted.
    text, status = render(
        "aaaaaaa",
        "bbbbbbb",
        versions(lockfile(("syn", "1.0.0"), ("syn", "2.0.0"))),
        versions(lockfile(("syn", "1.0.0"))),
    )
    expect("joined status", status, "0 added, 0 removed, 1 updated")
    expect("joined row", "| syn | 1.0.0, 2.0.0 | 1.0.0 |" in text, True)

    # The cap: 51 rows print 50 and one `_and 1 more_`.
    many = versions(lockfile(*[(f"crate-{index:02d}", "1.0.0") for index in range(51)]))
    text, status = render("aaaaaaa", "bbbbbbb", versions("version = 3\n"), many)
    expect("cap status", status, "51 added, 0 removed, 0 updated")
    expect("cap rows", text.count("| crate-") , 50)
    expect("cap note", "_and 1 more_" in text, True)

    # A malformed lockfile is unreadable, not empty: exit 3, never a clean summary.
    try:
        versions("version = 3\n[[package]\nname = \"clio\"\n")
        expect("malformed raises", "no exception", "Unreadable")
    except Unreadable:
        pass

    with tempfile.TemporaryDirectory(prefix="lockfile-diff-") as scratch:
        repo = Path(scratch)
        keep = ROOT
        try:
            globals()["ROOT"] = repo

            def run(*args: str) -> None:
                subprocess.run(args, cwd=repo, capture_output=True, check=True)

            run("git", "init", "-q")
            run("git", "config", "user.email", "t@example.invalid")
            run("git", "config", "user.name", "t")
            (repo / "Cargo.lock").write_text(one)
            run("git", "add", "Cargo.lock")
            run("git", "commit", "-q", "-m", "base")
            base = git("rev-parse", "HEAD").strip()
            (repo / "Cargo.lock").write_text(lockfile(("clio", "0.1.0"), ("serde", "1.0.1"), ("tokio", "1.0.0")))
            run("git", "add", "Cargo.lock")
            run("git", "commit", "-q", "-m", "head")
            head = git("rev-parse", "HEAD").strip()

            text, status = summary(base, head, "Cargo.lock")
            expect("two revisions status", status, "1 added, 0 removed, 1 updated")
            expect("two revisions added row", "| tokio | 1.0.0 |" in text, True)
            expect("two revisions update row", "| serde | 1.0.0 | 1.0.1 |" in text, True)
            expect("short revisions", f"`{base[:7]}` to `{head[:7]}`" in text, True)
            expect("same revision is no change", summary(head, head, "Cargo.lock")[1], "0 added, 0 removed, 0 updated")

            try:
                summary(base, head, "NoSuch.lock")
                expect("missing lockfile", "a summary", "Unreadable")
            except Unreadable:
                pass
            try:
                summary("deadbeefdeadbeef", head, "Cargo.lock")
                expect("bad revision", "a summary", "Unreadable")
            except Unreadable:
                pass

            out = repo / "summary.md"
            code = main(["--base", base, "--head", head, "--output", str(out)])
            expect("output writes the file", (code, out.is_file()), (EXIT_OK, True))
            expect("file holds the markdown", "## Cargo.lock diff" in out.read_text(), True)
        finally:
            globals()["ROOT"] = keep

    expect("bad argument", main(["--nope"]), EXIT_USAGE)
    expect("missing base", main([]), EXIT_USAGE)
    expect("head without a value", main(["--base", "HEAD", "--head"]), EXIT_USAGE)

    print("lockfile-diff self-test: failed" if failed else "lockfile-diff self-test: all passed")
    return 1 if failed else 0


def main(argv: list[str]) -> int:
    if argv == ["--self-test"]:
        return self_test()
    base: str | None = None
    head = DEFAULT_HEAD
    path = DEFAULT_PATH
    output: str | None = None
    rest = argv
    while rest:
        flag, *rest = rest
        if flag == "--base" and rest:
            base, *rest = rest
        elif flag == "--head" and rest:
            head, *rest = rest
        elif flag == "--path" and rest:
            path, *rest = rest
        elif flag == "--output" and rest:
            output, *rest = rest
        else:
            print(USAGE, file=sys.stderr)
            return EXIT_USAGE
    if base is None:
        print(USAGE, file=sys.stderr)
        return EXIT_USAGE
    try:
        text, status = summary(base, head, path)
    except Unreadable as error:
        print(f"lockfile-diff: {base}..{head} {path} could not be read: {error}", file=sys.stderr)
        return EXIT_UNREADABLE
    if output is None:
        write(sys.stdout, text)
        print(f"lockfile-diff: {status}")
        return EXIT_OK
    try:
        Path(output).write_text(text)
    except OSError as error:
        print(f"lockfile-diff: {output} could not be written ({error})", file=sys.stderr)
        return EXIT_UNREADABLE
    print(f"lockfile-diff: {status}", file=sys.stderr)
    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
