#!/usr/bin/env python3
"""Explain how the published MCP tool surface changed between two revisions.

The committed snapshot at `crates/clio-mcp/tests/fixtures/schema_pack.json` is the published surface as a file: one entry per tool, carrying its `description`, its `required` list, and one entry per parameter. A Rust test fails the build when that pack drifts from the code, so the pack is already the gate; what a reviewer lacks is the sentence saying what moved. This script reads the snapshot at two revisions and writes that sentence as markdown for `$GITHUB_STEP_SUMMARY`.

Both revisions are read with `git show <rev>:<path>` rather than from the working tree, so the summary describes the two commits rather than whatever is checked out. The comparison itself is a pure function over two parsed packs, which is why the self-test states every change shape as data instead of as a commit.

A tool is matched by its `name`. An added or a removed tool is listed with its description, because a new tool is the claim the reviewer has to judge. A tool in both revisions contributes one row per change: a changed `description`, a changed `required` list, a parameter added, a parameter removed, and per parameter kept in both, a changed `type` or a changed `description`. The count in the summary line is the number of changed rows, so it measures the work left rather than the number of tools touched.

A description is quoted to 120 characters, with an ellipsis when it was cut, because one parameter description here is a paragraph. Each table stops at 50 rows and states how many it left out. A snapshot that is new in this change is not a failure: when the file exists at the head revision and not at the base, there is nothing to compare, so the summary says so in one sentence and the exit is 0, because the pull request that introduces the snapshot is exactly the one with no base side.

Usage:
  python3 scripts/prompt_surface_diff.py --base <rev> [--head <rev>] [--snapshot <path>] [--output FILE]
  python3 scripts/prompt_surface_diff.py --self-test

--head defaults to HEAD and --snapshot to crates/clio-mcp/tests/fixtures/schema_pack.json. The markdown goes to --output when it is given and to stdout otherwise; the one-line `prompt-surface: N tool(s), A added, R removed, C changed` status is printed last, on stderr when a file was written and on stdout when the markdown went there.

A snapshot that is new in this change is not a failure. When the file exists at the head revision and not at the base, there is nothing to compare: the summary says so in one sentence and the exit is 0, because the pull request that introduces the snapshot is exactly the one that has no base side.

Exit codes:
  0  a summary was produced, whether or not the surface changed, including when the snapshot exists only at the head revision
  2  usage error
  3  a revision is unknown, or the snapshot is missing at the head revision or is not valid JSON, so no summary describes it
"""

from __future__ import annotations

import contextlib
import io
import json
import subprocess
import sys
import tempfile
from pathlib import Path

EXIT_OK = 0
EXIT_USAGE = 2
EXIT_UNREADABLE = 3

SUMMARY_TITLE = "## MCP tool schema surface"

#: The repository this script runs in. The self-test points it at a temporary
#: repository, so every git call goes through it.
ROOT = Path(__file__).resolve().parent.parent

DEFAULT_SNAPSHOT = "crates/clio-mcp/tests/fixtures/schema_pack.json"
DEFAULT_HEAD = "HEAD"

#: Rows per table, and the width a quoted description is cut to.
MAX_ROWS = 50
QUOTE = 120
SHORT = 7
ELLIPSIS = "..."

USAGE = "usage: prompt_surface_diff.py --base <rev> [--head <rev>] [--snapshot <path>] [--output FILE] | --self-test"


class Unreadable(Exception):
    """A revision, or the snapshot in it, could not be read or is not valid JSON."""


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


def snapshot_text(rev: str, path: str) -> str:
    """One revision's snapshot, as git stored it."""
    return git("show", f"{rev}:{path}")


def parse(text: str) -> tuple[str | None, dict[str, dict]]:
    """The catalog hash and the tools of one pack, by name, or Unreadable."""
    try:
        data = json.loads(text)
    except json.JSONDecodeError as error:
        raise Unreadable(f"the snapshot is not valid JSON ({error})") from error
    if not isinstance(data, dict):
        raise Unreadable("the snapshot is not a JSON object, so it carries no tools")
    entries = data.get("tools")
    if not isinstance(entries, list):
        raise Unreadable("the snapshot has no `tools` array, so it carries no surface to compare")
    found: dict[str, dict] = {}
    for entry in entries:
        if isinstance(entry, dict) and isinstance(entry.get("name"), str):
            found[entry["name"]] = entry
    digest = data.get("catalog_hash")
    return (digest if isinstance(digest, str) else None), found


def schema(tool: dict) -> dict:
    """The `inputSchema` of one tool, or an empty mapping when it has none."""
    inner = tool.get("inputSchema")
    return inner if isinstance(inner, dict) else {}


def required_names(tool: dict) -> list[str]:
    """The `required` list of one tool, as strings, in the order it gives."""
    values = schema(tool).get("required")
    return [value for value in values if isinstance(value, str)] if isinstance(values, list) else []


def parameters(tool: dict) -> dict[str, dict]:
    """The properties of one tool, as name to schema."""
    values = schema(tool).get("properties")
    if not isinstance(values, dict):
        return {}
    return {name: value for name, value in values.items() if isinstance(value, dict)}


def phrase(value) -> str:
    """A field's text, or the empty string when it is absent or not a string."""
    return value if isinstance(value, str) else ""


def quote(text: str) -> str:
    """One description on one line, cut to QUOTE characters with an ellipsis."""
    flat = " ".join(text.split())
    if len(flat) <= QUOTE:
        return flat
    return flat[: QUOTE - len(ELLIPSIS)].rstrip() + ELLIPSIS


def cell(text: str) -> str:
    """One table cell: quoted, and with the pipe that would end the cell escaped."""
    return quote(text).replace("|", "\\|")


def changes(name: str, before: dict, after: dict) -> list[list[str]]:
    """Every row that describes how one tool, present in both packs, changed."""
    rows: list[list[str]] = []
    if phrase(before.get("description")) != phrase(after.get("description")):
        rows.append([name, "description changed"])
    if required_names(before) != required_names(after):
        rows.append([name, "required list changed"])
    old_parameters, new_parameters = parameters(before), parameters(after)
    for parameter in sorted(new_parameters.keys() - old_parameters.keys()):
        rows.append([name, f"parameter `{parameter}` added"])
    for parameter in sorted(old_parameters.keys() - new_parameters.keys()):
        rows.append([name, f"parameter `{parameter}` removed"])
    for parameter in sorted(old_parameters.keys() & new_parameters.keys()):
        old, new = old_parameters[parameter], new_parameters[parameter]
        if phrase(old.get("type")) != phrase(new.get("type")):
            rows.append([name, f"parameter `{parameter}` type changed"])
        if phrase(old.get("description")) != phrase(new.get("description")):
            rows.append([name, f"parameter `{parameter}` description changed"])
    return rows


def compare(before: dict[str, dict], after: dict[str, dict]) -> tuple[list[str], list[str], list[list[str]]]:
    """The added tools, the removed tools, and every changed row, each sorted."""
    added = sorted(after.keys() - before.keys())
    removed = sorted(before.keys() - after.keys())
    rows = [row for name in sorted(before.keys() & after.keys()) for row in changes(name, before[name], after[name])]
    return added, removed, rows


def table(title: str, header: list[str], rows: list[list[str]]) -> list[str]:
    """One markdown table, capped, with the number of omitted rows stated."""
    lines = ["", f"### {title}", "", "| " + " | ".join(header) + " |", "| " + " | ".join("---" for _ in header) + " |"]
    lines += ["| " + " | ".join(row) + " |" for row in rows[:MAX_ROWS]]
    if len(rows) > MAX_ROWS:
        lines += ["", f"_and {len(rows) - MAX_ROWS} more_"]
    return lines


def render(
    base_rev: str,
    head_rev: str,
    before: tuple[str | None, dict[str, dict]],
    after: tuple[str | None, dict[str, dict]],
) -> tuple[str, str]:
    """The markdown summary and its status line, for two parsed packs."""
    base_digest, base_tools = before
    head_digest, head_tools = after
    added, removed, rows = compare(base_tools, head_tools)
    status = f"{len(head_tools)} tool(s), {len(added)} added, {len(removed)} removed, {len(rows)} changed"
    lines = [SUMMARY_TITLE, "", f"`{base_rev}` to `{head_rev}`: {status}."]
    if base_digest and head_digest:
        lines += [f"catalog hash: `{base_digest}` to `{head_digest}`."]
    if not (added or removed or rows):
        lines += ["", "No tool, description, or parameter change."]
        return "\n".join(lines) + "\n", status
    described = [[name, cell(phrase(head_tools[name].get("description")))] for name in added]
    retired = [[name, cell(phrase(base_tools[name].get("description")))] for name in removed]
    lines += table("Added tools", ["tool", "description"], described)
    lines += table("Removed tools", ["tool", "description"], retired)
    lines += table("Changed tools", ["tool", "change"], rows)
    return "\n".join(lines) + "\n", status


def in_tree(rev: str, path: str) -> bool:
    """Whether the snapshot is in one revision's tree, which a new file is not."""
    try:
        git("cat-file", "-e", f"{rev}:{path}")
    except Unreadable:
        return False
    return True


def summary(base: str, head: str, snapshot: str) -> tuple[str, str]:
    """The markdown and its status line for two revisions, or Unreadable."""
    base_short, head_short = short_rev(base), short_rev(head)
    if not in_tree(head, snapshot):
        raise Unreadable(f"the snapshot does not exist at {head_short}")
    if not in_tree(base, snapshot):
        sentence = f"the snapshot does not exist at {base_short}; the tool surface is new in this change."
        return f"{SUMMARY_TITLE}\n\nprompt-surface: {sentence}\n", sentence
    return render(base_short, head_short, parse(snapshot_text(base, snapshot)), parse(snapshot_text(head, snapshot)))


def write(stream, text: str) -> None:
    """One write to a stream, so a long table is not interleaved with anything."""
    stream.write(text)


def self_test() -> int:
    """The comparison, every change shape, the rendering, and each exit code."""
    failed = 0

    def expect(label: str, got, want) -> None:
        nonlocal failed
        if got != want:
            failed += 1
            print(f"prompt-surface self-test: {label}: expected {want!r}, got {got!r}")

    def parameter(kind: str = "string", description: str = "A parameter.") -> dict:
        return {"type": kind, "description": description}

    def tool(name: str, description: str = "Does a thing.", required: list[str] | None = None, properties: dict | None = None) -> dict:
        return {"name": name, "description": description, "inputSchema": {"properties": properties or {}, "required": required or []}}

    def pack(*tools: dict, catalog_hash: str = "fnv1a64:aaaa") -> str:
        return json.dumps({"catalog_hash": catalog_hash, "format": "am_tool_schema_pack", "tools": list(tools)})

    def diff(base_pack: str, head_pack: str) -> tuple[str, str]:
        return render("aaaaaaa", "bbbbbbb", parse(base_pack), parse(head_pack))

    def section(text: str, title: str) -> str:
        return text.split(f"### {title}", 1)[1].split("###", 1)[0] if f"### {title}" in text else ""

    recall = tool("recall", "Recall memories.", ["bank"], {"bank": parameter()})
    store = tool("store", "Store a memory.", ["bank"], {"bank": parameter()})

    # No change: a summary is produced, states it, and prints no table.
    text, status = diff(pack(recall), pack(recall))
    expect("no change status", status, "1 tool(s), 0 added, 0 removed, 0 changed")
    expect("no change title", text.splitlines()[0], SUMMARY_TITLE)
    for label, needle in (("no change counts line", "`aaaaaaa` to `bbbbbbb`: 1 tool(s), 0 added, 0 removed, 0 changed."), ("no change hash line", "catalog hash: `fnv1a64:aaaa` to `fnv1a64:aaaa`."), ("no change sentence", "No tool, description, or parameter change.")):
        expect(label, needle in text, True)
    expect("no change prints no table and no absent hash", ("### Added tools" in text or "| tool |" in text, "catalog hash" in diff('{"tools": []}', '{"tools": []}')[0]), (False, False))

    # One tool added, and nothing else.
    text, status = diff(pack(recall), pack(recall, store, catalog_hash="fnv1a64:bbbb"))
    expect("added status", status, "2 tool(s), 1 added, 0 removed, 0 changed")
    expect("added section", "| store | Store a memory. |" in section(text, "Added tools"), True)
    for label, needle in (("added row", "| store | Store a memory. |"), ("added hash pair", "catalog hash: `fnv1a64:aaaa` to `fnv1a64:bbbb`.")):
        expect(label, needle in text, True)
    for title in ("Changed tools", "Removed tools"):
        expect(f"added leaves {title} empty", "| store |" in section(text, title), False)

    # One tool removed: the base side is the one described.
    text, status = diff(pack(recall, store), pack(recall))
    expect("removed status", status, "1 tool(s), 0 added, 1 removed, 0 changed")
    expect("removed section holds that tool and only it", ("| store | Store a memory. |" in section(text, "Removed tools"), "| recall |" in section(text, "Removed tools")), (True, False))

    # A changed required list, in either direction and in a different order.
    text, status = diff(pack(tool("store", required=["bank"])), pack(tool("store", required=["bank", "text"])))
    expect("required status", status, "1 tool(s), 0 added, 0 removed, 1 changed")
    expect("required row", "| store | required list changed |" in section(text, "Changed tools"), True)
    expect("required order", "required list changed" in diff(pack(tool("s", required=["a", "b"])), pack(tool("s", required=["b", "a"])))[0], True)

    # A parameter added, and one removed, in the same step.
    text, status = diff(
        pack(tool("store", properties={"bank": parameter(), "text": parameter()})),
        pack(tool("store", properties={"bank": parameter(), "mission": parameter()})),
    )
    expect("parameter set status", status, "1 tool(s), 0 added, 0 removed, 2 changed")
    for label, needle in (("added", "| store | parameter `mission` added |"), ("removed", "| store | parameter `text` removed |")):
        expect(f"parameter {label} row", needle in text, True)

    # One changed field per case: the tool description, a parameter type, a parameter description.
    for label, before_tool, after_tool, row in (
        ("description", tool("store"), tool("store", "Store a memory, with a bank."), "| store | description changed |"),
        ("type", tool("store", properties={"bank": parameter("string", "The bank.")}), tool("store", properties={"bank": parameter("number", "The bank.")}), "| store | parameter `bank` type changed |"),
        ("parameter description", tool("store", properties={"bank": parameter("string", "The bank.")}), tool("store", properties={"bank": parameter("string", "The bank, or the connection default.")}), "| store | parameter `bank` description changed |"),
    ):
        text, status = diff(pack(before_tool), pack(after_tool))
        expect(f"{label} status", status, "1 tool(s), 0 added, 0 removed, 1 changed")
        expect(f"{label} row", row in text, True)

    # The 120-character cut, with the ellipsis that says it was cut.
    long_description = "x" * 130
    expect("cut is QUOTE wide and ends with an ellipsis", (len(quote(long_description)), quote(long_description)), (QUOTE, "x" * (QUOTE - len(ELLIPSIS)) + ELLIPSIS))
    expect("short description untouched", quote("Store a memory."), "Store a memory.")
    text, _ = diff(pack(), pack(tool("store", long_description)))
    expect("the table holds the cut, not the full text", ("x" * (QUOTE - len(ELLIPSIS)) + ELLIPSIS in text, long_description in text), (True, False))
    expect("a pipe cannot end a cell", "| store | a \\| b |" in diff(pack(), pack(tool("store", "a | b")))[0], True)

    # The cap: 51 added tools print 50 rows and one `_and 1 more_`.
    many = pack(*[tool(f"tool-{index:02d}") for index in range(51)])
    text, status = diff(pack(), many)
    expect("cap status", status, "51 tool(s), 51 added, 0 removed, 0 changed")
    expect("cap rows", text.count("| tool-"), 50)
    expect("cap note", "_and 1 more_" in text, True)

    # A malformed snapshot is unreadable, not empty.
    for label, malformed in (("malformed raises", '{"tools": ['), ("no tools array raises", '{"format": "am_tool_schema_pack"}')):
        try:
            parse(malformed)
            expect(label, "no exception", "Unreadable")
        except Unreadable:
            pass

    def captured(*args: str) -> tuple[int, str, str]:
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = main(list(args))
        return code, out.getvalue(), err.getvalue()

    # A real repository, so the snapshot is read out of a commit and not the tree.
    with tempfile.TemporaryDirectory(prefix="prompt-surface-") as scratch:
        repo = Path(scratch)
        keep = ROOT
        try:
            globals()["ROOT"] = repo

            def run(*args: str) -> None:
                subprocess.run(args, cwd=repo, capture_output=True, check=True)

            snapshot = Path(DEFAULT_SNAPSHOT)
            target = repo / snapshot
            target.parent.mkdir(parents=True, exist_ok=True)
            run("git", "init", "-q")
            run("git", "config", "user.email", "t@example.invalid")
            run("git", "config", "user.name", "t")

            def commit(message: str, text: str | None = None) -> str:
                if text is not None:
                    target.write_text(text)
                    run("git", "add", str(snapshot))
                run("git", "commit", "-q", "--allow-empty", "-m", message)
                return git("rev-parse", "HEAD").strip()

            # The first commit carries no snapshot at all, which is the state a change that adds one starts from.
            empty = commit("empty")
            base = commit("base", pack(tool("retired", "Retired tool.", [], {"bank": parameter()})))
            head = commit("head", pack(recall, store, catalog_hash="fnv1a64:cccc"))

            code, out, _ = captured("--base", base, "--head", head)
            expect("two revisions exit", code, EXIT_OK)
            for needle in (f"`{base[:SHORT]}` to `{head[:SHORT]}`: 2 tool(s), 2 added, 1 removed, 0 changed.", "prompt-surface: 2 tool(s), 2 added, 1 removed, 0 changed", "| recall | Recall memories. |", "| retired | Retired tool. |"):
                expect(f"two revisions prints {needle!r}", needle in out, True)
            expect("same revision is no change", captured("--base", head, "--head", head)[1].count("No tool, description"), 1)

            # The snapshot is new in this change: nothing to compare, so the summary says that and the exit is 0.
            code, out, _ = captured("--base", empty, "--head", head)
            expect("new snapshot summary", (code, out.splitlines()[0]), (EXIT_OK, SUMMARY_TITLE))
            expect("new snapshot sentence, once in the summary and once as the status line", out.count(f"prompt-surface: the snapshot does not exist at {empty[:SHORT]}; the tool surface is new in this change."), 2)
            # Missing at the head revision is still unreadable, because there is no surface to describe.
            code, _, err = captured("--base", empty, "--head", empty)
            expect("missing at head exit", code, EXIT_UNREADABLE)
            expect("missing at head says which", f"the snapshot does not exist at {empty[:SHORT]}" in err, True)

            out_file = repo / "summary.md"
            code, out, err = captured("--base", base, "--head", head, "--output", str(out_file))
            expect("output exit and file", (code, SUMMARY_TITLE in out_file.read_text()), (EXIT_OK, True))
            expect("output leaves stdout empty", out, "")
            expect("output states the status on stderr", err.strip(), "prompt-surface: 2 tool(s), 2 added, 1 removed, 0 changed")

            # A snapshot that is not valid JSON, read out of a commit.
            code, _, err = captured("--base", base, "--head", commit("broken", '{"tools": ['))
            expect("invalid JSON exit", code, EXIT_UNREADABLE)
            expect("invalid JSON says so", "not valid JSON" in err, True)
            expect("invalid JSON prints no summary", "## MCP" in err, False)

            code, _, err = captured("--base", base, "--head", head, "--snapshot", "NoSuch.json")
            expect("missing snapshot exit", code, EXIT_UNREADABLE)
            code, _, _ = captured("--base", "deadbeefdeadbeef", "--head", head)
            expect("bad revision exit", code, EXIT_UNREADABLE)
        finally:
            globals()["ROOT"] = keep

    for args in (("--nope",), (), ("--base", "HEAD", "--head")):
        expect(f"usage error for {args}", captured(*args)[0], EXIT_USAGE)

    print("prompt-surface self-test: failed" if failed else "prompt-surface self-test: all passed")
    return 1 if failed else 0


def main(argv: list[str]) -> int:
    if argv == ["--self-test"]:
        return self_test()
    base: str | None = None
    head = DEFAULT_HEAD
    snapshot = DEFAULT_SNAPSHOT
    output: str | None = None
    rest = argv
    while rest:
        flag, *rest = rest
        if flag == "--base" and rest:
            base, *rest = rest
        elif flag == "--head" and rest:
            head, *rest = rest
        elif flag == "--snapshot" and rest:
            snapshot, *rest = rest
        elif flag == "--output" and rest:
            output, *rest = rest
        else:
            print(USAGE, file=sys.stderr)
            return EXIT_USAGE
    if base is None:
        print(USAGE, file=sys.stderr)
        return EXIT_USAGE
    try:
        text, status = summary(base, head, snapshot)
    except Unreadable as error:
        print(f"prompt-surface: {base}..{head} {snapshot} could not be read: {error}", file=sys.stderr)
        return EXIT_UNREADABLE
    if output is None:
        write(sys.stdout, text)
        print(f"prompt-surface: {status}")
        return EXIT_OK
    try:
        Path(output).write_text(text)
    except OSError as error:
        print(f"prompt-surface: {output} could not be written ({error})", file=sys.stderr)
        return EXIT_UNREADABLE
    print(f"prompt-surface: {status}", file=sys.stderr)
    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
