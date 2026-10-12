#!/usr/bin/env python3
"""One changelog file per change, collected into CHANGELOG.md at release time.

Every pull request that a reader would notice adds one fragment under changelog.d/, named <slug>.<category>.md, for example add-user-authentication.feature.md. The release build groups the fragments under the Keep a Changelog headings they map to, writes one release section, and deletes the consumed files, so two pull requests never edit the same file.

Usage:
  python3 scripts/changelog_fragments.py --check [--base <ref>]
  python3 scripts/changelog_fragments.py --build --version <X.Y.Z> [--date <YYYY-MM-DD>]
  python3 scripts/changelog_fragments.py --self-test

The check always validates the fragments on disk: the name shape, the category, and real prose. With --base (or CHANGELOG_BASE, which CI sets from the diff base) it also reads the diff: product code changed with no fragment added fails, and CHANGELOG.md edited with no fragment change fails, because normal pull requests never touch that file by hand. Without a base only the files on disk are judged.

Exit codes:
  0  the check passed, or the build wrote the release section
  1  at least one finding, each naming the file and the rule
  2  usage error
  3  a file this check had to read could not be read, so the clean parts of the run do not cover it
"""

from __future__ import annotations

import datetime
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

EXIT_OK = 0
EXIT_FINDINGS = 1
EXIT_USAGE = 2
EXIT_UNREADABLE = 3

ROOT = Path(__file__).resolve().parent.parent
FRAGMENT_DIR = ROOT / "changelog.d"
CHANGELOG = ROOT / "CHANGELOG.md"

CATEGORIES = {"feature": "Added", "bugfix": "Fixed", "improvement": "Changed"}
SECTION_ORDER = ("Added", "Changed", "Fixed")

NAME = re.compile(r"^([a-z0-9]+(?:-[a-z0-9]+)*)\.(feature|bugfix|improvement)\.md$")
ISSUES = re.compile(r"issues-(\d+(?:-\d+)*)$")
VERSION = re.compile(r"^\d+\.\d+\.\d+$")
UNRELEASED = re.compile(r"^## \[Unreleased\]")
HEADING = re.compile(r"^##(?!#)")
FENCE = "```"

IGNORED = {"README.md", ".gitkeep", ".gitignore"}
MIN_SLUG = 8
MIN_BODY = 12

SKIP_MARKER = "No user-facing change."
TRIAGE_WORDS = ("security", "deprecat", "remov", "breaking", "cve", "yank")

CODE_PREFIXES = ("crates/", "sql/")
FRAGMENT_PREFIX = "changelog.d/"
CHANGELOG_NAME = "CHANGELOG.md"

GITHUB_REPO = "https://github.com/agentmemoir/clio"

USAGE = "usage: changelog_fragments.py --check [--base <ref>] | --build --version <X.Y.Z> [--date <YYYY-MM-DD>] | --self-test"


def fragments() -> list[Path]:
    """The fragments on disk, sorted, without the directory's own documentation."""
    if not FRAGMENT_DIR.is_dir():
        return []
    return sorted(path for path in FRAGMENT_DIR.iterdir() if path.is_file() and path.name not in IGNORED)


def validate_name(path: Path) -> str | None:
    """The naming rule, or None when the name holds."""
    match = NAME.match(path.name)
    if match is None:
        return f"{path.name}: name it <slug>.<category>.md, for example fix-crash-on-startup.bugfix.md, with category feature, bugfix, or improvement"
    if len(match.group(1)) < MIN_SLUG:
        return f"{path.name}: the slug is {len(match.group(1))} characters, at least {MIN_SLUG}, so the name says what changed"
    return None


def validate_body(text: str, name: str) -> list[str]:
    """The prose rules: one real paragraph, and no hard-wrapped continuation line."""
    findings = []
    stripped = text.strip()
    if len(stripped) < MIN_BODY:
        findings.append(f"{name}: write at least {MIN_BODY} characters of user-facing prose, on one line")
        return findings
    in_fence = False
    for line in text.splitlines():
        if line.startswith(FENCE):
            in_fence = not in_fence
        elif not in_fence and line[:1] in (" ", "\t"):
            findings.append(f"{name}: one paragraph per line, so a wrapped continuation line is not finished")
            break
    return findings


def validate_tree() -> list[str]:
    """Every fragment on disk, named and written to the rule."""
    findings = []
    seen: dict[str, str] = {}
    for path in fragments():
        lowered = path.name.lower()
        if lowered in seen:
            findings.append(f"{path.name}: duplicates {seen[lowered]} on a case-insensitive filesystem")
            continue
        seen[lowered] = path.name
        problem = validate_name(path)
        if problem is not None:
            findings.append(problem)
            continue
        try:
            findings.extend(validate_body(path.read_text(), path.name))
        except OSError as error:
            findings.append(f"{path.name}: could not be read ({error})")
    return findings


def is_test_path(path: str) -> bool:
    """Test, fixture, and bench files, which never need a changelog fragment."""
    parts = path.split("/")
    return any(part in ("tests", "fixtures", "benches") for part in parts[:-1]) or parts[-1].endswith("_tests.rs")


def diff_status(base: str) -> tuple[dict[str, str], str | None]:
    """The changed paths between base and the worktree, with git statuses, or an error."""
    try:
        proc = subprocess.run(
            ["git", "diff", "--name-status", base],
            capture_output=True,
            text=True,
            check=False,
            cwd=ROOT,
        )
    except OSError as error:
        return {}, f"git could not run ({error})"
    if proc.returncode != 0:
        return {}, f"git diff failed for base {base}: {proc.stderr.strip()}"
    changes: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) == 3:
            changes[parts[2]] = parts[0][:1]
        elif len(parts) == 2:
            changes[parts[1]] = parts[0][:1]
    try:
        proc = subprocess.run(
            ["git", "ls-files", "--others", "--exclude-standard", "--", FRAGMENT_PREFIX],
            capture_output=True,
            text=True,
            check=False,
            cwd=ROOT,
        )
    except OSError:
        proc = None
    if proc is not None and proc.returncode == 0:
        for line in proc.stdout.splitlines():
            changes.setdefault(line, "A")
    return changes, None


def requires_fragment(changes: dict[str, str]) -> tuple[bool, bool]:
    """Whether the diff needs a fragment it has none of, and whether it edits the changelog by hand."""
    fragment_touched = any(path.startswith(FRAGMENT_PREFIX) for path in changes)
    fragment_added = any(path.startswith(FRAGMENT_PREFIX) and status in ("A", "M", "R", "C") for path, status in changes.items())
    code_changed = any(path.startswith(prefix) and not is_test_path(path) for path in changes for prefix in CODE_PREFIXES)
    direct_edit = CHANGELOG_NAME in changes and not fragment_touched
    return code_changed and not fragment_added, direct_edit


def check(base: str | None) -> tuple[list[str], int]:
    """The fragments on disk, plus the diff rules when a base is known."""
    findings = validate_tree()
    if base is None:
        return findings, EXIT_FINDINGS if findings else EXIT_OK
    changes, error = diff_status(base)
    if error is not None:
        return [error], EXIT_UNREADABLE
    missing, direct = requires_fragment(changes)
    if missing:
        findings.append(f"product code changed with no fragment added under {FRAGMENT_PREFIX}; add one or the change ships undescribed")
    if direct:
        findings.append(f"{CHANGELOG_NAME} edited by hand; add a fragment under {FRAGMENT_PREFIX} instead")
    return findings, EXIT_FINDINGS if findings else EXIT_OK


def today_utc() -> str:
    """Today's date in UTC as YYYY-MM-DD."""
    return datetime.datetime.now(datetime.timezone.utc).date().isoformat()


def issue_links(stem: str) -> str:
    """The [Issue #N] links a slug's -issues- marker carries, or nothing."""
    match = ISSUES.search(stem)
    if match is None:
        return ""
    return "".join(f" [Issue #{number}]({GITHUB_REPO}/issues/{number})" for number in match.group(1).split("-"))


def triage_hits() -> list[str]:
    """Fragments whose words suggest a Security, Deprecated, or Removed heading, for a human to place."""
    hits = []
    for path in fragments():
        try:
            text = f"{path.name} {path.read_text()}".lower()
        except OSError:
            continue
        if any(word in text for word in TRIAGE_WORDS):
            hits.append(path.name)
    return hits


def entry_text(path: Path, body: str) -> str:
    """One fragment as one changelog bullet, with its issue links on the end."""
    if body.startswith(("- ", "* ")):
        body = body[2:].lstrip()
    match = NAME.match(path.name)
    stem = match.group(1) if match else path.stem
    return f"- {body}{issue_links(stem)}"


def build(version: str, date: str) -> tuple[list[str], int]:
    """The fragments grouped under their headings, written as one release section."""
    findings = validate_tree()
    if findings:
        return findings, EXIT_FINDINGS
    frags = fragments()
    if not frags:
        return ["no fragments under changelog.d/, so there is nothing to release"], EXIT_FINDINGS
    try:
        text = CHANGELOG.read_text()
    except OSError as error:
        return [f"{CHANGELOG_NAME} could not be read ({error})"], EXIT_UNREADABLE
    if re.search(rf"^## \[{re.escape(version)}\]", text, re.MULTILINE):
        return [f"## [{version}] already heads a section, so the build refuses to write it twice"], EXIT_FINDINGS
    lines = text.splitlines()
    unreleased = next((index for index, line in enumerate(lines) if UNRELEASED.match(line)), None)
    if unreleased is None:
        return [f"{CHANGELOG_NAME} has no ## [Unreleased] section to release under"], EXIT_FINDINGS
    following = next((index for index in range(unreleased + 1, len(lines)) if HEADING.match(lines[index])), len(lines))
    groups: dict[str, list[str]] = {section: [] for section in SECTION_ORDER}
    skipped = 0
    for path in frags:
        try:
            body = path.read_text().strip()
        except OSError as error:
            return [f"{path.name} could not be read ({error})"], EXIT_UNREADABLE
        if body == SKIP_MARKER:
            skipped += 1
            continue
        groups[CATEGORIES[NAME.match(path.name).group(2)]].append(entry_text(path, body))
    section = [f"## [{version}] - {date}", ""]
    for heading in SECTION_ORDER:
        if groups[heading]:
            section += [f"### {heading}", ""] + sorted(groups[heading]) + [""]
    insert = "\n".join(section)
    before = "\n".join(lines[:following]).rstrip("\n")
    after = "\n".join(lines[following:])
    CHANGELOG.write_text(f"{before}\n\n{insert}\n{after}" if after else f"{before}\n\n{insert}\n")
    triage = triage_hits()
    for path in frags:
        path.unlink()
    count = sum(len(entries) for entries in groups.values())
    message = f"changelog: wrote {count} entries under [{version}], removed {len(frags)} fragments"
    if skipped:
        message += f" ({skipped} marked no user-facing change)"
    print(message)
    if triage:
        print(f"changelog: triage candidates for Security, Deprecated, or Removed: {', '.join(triage)}")
    return [], EXIT_OK


def self_test() -> int:
    """The naming, prose, diff, and build rules against stand-ins."""
    failed = 0

    def expect(label: str, got, want) -> None:
        nonlocal failed
        if got != want:
            failed += 1
            print(f"changelog self-test: {label}: expected {want!r}, got {got!r}")

    expect("good name", validate_name(Path("fix-crash-on-startup.bugfix.md")), None)
    expect("issue name", validate_name(Path("fix-session-expiration-issues-45-46.bugfix.md")), None)
    expect("bad suffix", validate_name(Path("fix-crash.todo.md")) is None, False)
    expect("short slug", validate_name(Path("fixit.bugfix.md")) is None, False)
    expect("upper case", validate_name(Path("Fix-Crash.bugfix.md")) is None, False)
    expect("short body", validate_body("Fixed it.", "x.bugfix.md") != [], True)
    expect("brief body", validate_body("Fixed crash.", "x.bugfix.md"), [])
    expect("good body", validate_body("Fixed session expiration handling.", "x.bugfix.md"), [])
    expect("wrapped body", validate_body("Fixed session expiration\n handling.", "x.bugfix.md") != [], True)
    expect("fenced body", validate_body("```\n indented\n```\nFixed session expiration handling.", "x.bugfix.md"), [])
    expect("links", issue_links("fix-session-expiration-issues-45-46"), f" [Issue #45]({GITHUB_REPO}/issues/45) [Issue #46]({GITHUB_REPO}/issues/46)")
    expect("no links", issue_links("add-user-authentication"), "")
    expect("mid-slug marker ignored", issue_links("retire-issues-2026-cleanup"), "")
    expect("date shape", bool(re.match(r"^\d{4}-\d{2}-\d{2}$", today_utc())), True)
    expect("test suffix", is_test_path("crates/c/src/backup_path_tests.rs"), True)
    expect("tests dir", is_test_path("crates/c/tests/roundtrip.rs"), True)
    expect("fixtures dir", is_test_path("crates/c/fixtures/x.txt"), True)
    expect("prod file", is_test_path("crates/c/src/main.rs"), False)

    missing, direct = requires_fragment({"crates/clio/src/main.rs": "M"})
    expect("code needs fragment", (missing, direct), (True, False))
    missing, direct = requires_fragment({"crates/clio/src/main.rs": "M", "changelog.d/fix-crash.bugfix.md": "A"})
    expect("fragment satisfies", (missing, direct), (False, False))
    missing, direct = requires_fragment({"CHANGELOG.md": "M"})
    expect("direct edit", (missing, direct), (False, True))
    missing, direct = requires_fragment({"CHANGELOG.md": "M", "changelog.d/fix-crash.bugfix.md": "D"})
    expect("release touches fragments", (missing, direct), (False, False))
    missing, direct = requires_fragment({"README.md": "M"})
    expect("docs need nothing", (missing, direct), (False, False))
    expect("test-only needs nothing", requires_fragment({"crates/c/src/backup_path_tests.rs": "M"}), (False, False))
    expect("fixtures need nothing", requires_fragment({"crates/c/fixtures/x.txt": "A"}), (False, False))
    expect("mixed needs fragment", requires_fragment({"crates/c/src/a_tests.rs": "M", "crates/c/src/b.rs": "M"}), (True, False))

    with tempfile.TemporaryDirectory() as tmp:
        keep = (FRAGMENT_DIR, CHANGELOG)
        try:
            stand = Path(tmp)
            globals()["FRAGMENT_DIR"] = stand / "changelog.d"
            globals()["CHANGELOG"] = stand / "CHANGELOG.md"
            FRAGMENT_DIR.mkdir()
            (FRAGMENT_DIR / "add-user-authentication.feature.md").write_text("Added support for user authentication.\n")
            (FRAGMENT_DIR / "fix-token-disclosure-issues-9.bugfix.md").write_text("Fixed a security token disclosure issue.\n")
            (FRAGMENT_DIR / "refresh-docs-index.improvement.md").write_text(f"{SKIP_MARKER}\n")
            (FRAGMENT_DIR / "README.md").write_text("ignored\n")
            CHANGELOG.write_text("# Changelog\n\n## [Unreleased]\n\n## [0.1.0] - 2026-10-10\n\n### Added\n\n- Old entry.\n")
            expect("tree valid", validate_tree(), [])
            expect("triage flags security", triage_hits(), ["fix-token-disclosure-issues-9.bugfix.md"])
            problems, code = build("0.2.0", "2026-10-12")
            expect("build ok", (problems, code), ([], EXIT_OK))
            body = CHANGELOG.read_text()
            expect("section heads", "## [0.2.0] - 2026-10-12" in body, True)
            expect("entry listed", "- Added support for user authentication." in body, True)
            expect("issue linked", "[Issue #9]" in body, True)
            expect("marker skipped", SKIP_MARKER in body, False)
            expect("fragment gone", (FRAGMENT_DIR / "add-user-authentication.feature.md").exists(), False)
            expect("marker gone", (FRAGMENT_DIR / "refresh-docs-index.improvement.md").exists(), False)
            expect("readme stays", (FRAGMENT_DIR / "README.md").exists(), True)
            problems, code = build("0.3.0", "2026-10-12")
            expect("empty build refused", code, EXIT_FINDINGS)
        finally:
            globals()["FRAGMENT_DIR"], globals()["CHANGELOG"] = keep

    with tempfile.TemporaryDirectory() as repo:
        keep_root = ROOT
        try:
            globals()["ROOT"] = Path(repo)
            run = lambda *args: subprocess.run(args, cwd=repo, capture_output=True, check=True)
            run("git", "init", "-q")
            run("git", "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "base")
            (Path(repo) / "crates").mkdir()
            (Path(repo) / "crates" / "prod.rs").write_text("x\n")
            run("git", "add", "crates/prod.rs")
            run("git", "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "prod")
            with open(Path(repo) / "crates" / "prod.rs", "a") as handle:
                handle.write("y\n")
            (Path(repo) / "changelog.d").mkdir()
            (Path(repo) / "changelog.d" / "fix-untracked-probe.bugfix.md").write_text("Fixed an untracked probe failure.\n")
            changes, error = diff_status("HEAD")
            expect("diff reads", error, None)
            expect("tracked edit seen", changes.get("crates/prod.rs"), "M")
            expect("untracked fragment seen", changes.get("changelog.d/fix-untracked-probe.bugfix.md"), "A")
        finally:
            globals()["ROOT"] = keep_root

    print("changelog self-test: failed" if failed else "changelog self-test: all passed")
    return 1 if failed else 0


def main(argv: list[str]) -> int:
    if argv == ["--self-test"]:
        return self_test()
    if argv[:1] == ["--check"]:
        base: str | None = None
        rest = argv[1:]
        if rest[:1] == ["--base"]:
            if len(rest) != 2:
                print(USAGE, file=sys.stderr)
                return EXIT_USAGE
            base = rest[1]
        elif rest:
            print(USAGE, file=sys.stderr)
            return EXIT_USAGE
        base = base or os.environ.get("CHANGELOG_BASE")
        try:
            findings, code = check(base)
        except OSError as error:
            print(f"changelog: {error}", file=sys.stderr)
            return EXIT_UNREADABLE
        for finding in findings:
            print(f"changelog: {finding}")
        if not findings:
            print("changelog: fragments valid" if base is None else "changelog: fragments valid, diff needs nothing more")
        return code
    if argv[:1] == ["--build"]:
        version: str | None = None
        date = today_utc()
        rest = argv[1:]
        while rest:
            flag, *rest = rest
            if flag == "--version" and rest and version is None:
                version, *rest = rest
            elif flag == "--date" and rest:
                date, *rest = rest
            else:
                print(USAGE, file=sys.stderr)
                return EXIT_USAGE
        if version is None or VERSION.match(version) is None:
            print("changelog: --version needs X.Y.Z", file=sys.stderr)
            return EXIT_USAGE
        if not re.match(r"^\d{4}-\d{2}-\d{2}$", date):
            print("changelog: --date needs YYYY-MM-DD", file=sys.stderr)
            return EXIT_USAGE
        try:
            findings, code = build(version, date)
        except OSError as error:
            print(f"changelog: {error}", file=sys.stderr)
            return EXIT_UNREADABLE
        for finding in findings:
            print(f"changelog: {finding}")
        return code
    print(USAGE, file=sys.stderr)
    return EXIT_USAGE


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
