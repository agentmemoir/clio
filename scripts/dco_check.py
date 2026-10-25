#!/usr/bin/env python3
"""The DCO check, called from `.github/workflows/dco.yml`.

Every non-merge commit in a pull request must carry a `Signed-off-by:` trailer
whose email matches the commit author email, compared without case. A commit
whose author is a bot is skipped, and so is the merge commit GitHub adds.

A maintainer fixes a bad commit by amending or rebasing it and pushing again.
There is no override, so the check cannot be waved through.

Environment: `GITHUB_REPOSITORY`, `GITHUB_EVENT_PATH`, `GITHUB_TOKEN` and
optionally `GITHUB_API_URL`. The token is never printed.

    python3 scripts/dco_check.py
    python3 scripts/dco_check.py --self-test

Exit codes: 0 every commit is signed off, 1 at least one is not, or a read
failed.
"""

from __future__ import annotations

import json
import os
import re
import sys
import urllib.error
import urllib.request

TRAILER = re.compile(r"^Signed-off-by:\s*(?P<name>.*?)\s*<(?P<email>[^<>]+)>\s*$", re.IGNORECASE)


def fail(message: str) -> None:
    """Print an annotation and stop the run."""
    print(f"::error::{message}")
    raise SystemExit(1)


def trailer_emails(message: str) -> set[str]:
    """The lowercased emails of every Signed-off-by trailer in a message."""
    emails = set()
    for line in message.splitlines():
        match = TRAILER.match(line.strip())
        if match:
            emails.add(match.group("email").strip().lower())
    return emails


def is_bot(login: str, kind: str) -> bool:
    """True for a GitHub App account."""
    return kind == "Bot" or login.endswith("[bot]")


def offender(commit: dict) -> str | None:
    """The reason this commit fails, or None when it passes."""
    sha = str(commit.get("sha", ""))
    author = commit.get("author") or {}
    login = str(author.get("login", ""))
    kind = str(author.get("type", ""))
    if is_bot(login, kind):
        return None
    detail = commit.get("commit") or {}
    parents = commit.get("parents") or []
    if len(parents) > 1:
        return None
    email = str((detail.get("author") or {}).get("email", "")).lower()
    emails = trailer_emails(str(detail.get("message", "")))
    if not emails:
        return f"no Signed-off-by trailer (author email {email or 'unknown'})"
    if email not in emails:
        found = ", ".join(sorted(emails))
        return f"Signed-off-by email {found} does not match the author email {email}"
    return None


def fetch_commits(api_root: str, repo: str, token: str, number: int) -> list[dict]:
    """Every commit in the pull request, or stop the run."""
    commits: list[dict] = []
    for page in range(1, 31):
        url = f"{api_root}/repos/{repo}/pulls/{number}/commits?per_page=100&page={page}"
        request = urllib.request.Request(url)
        request.add_header("Accept", "application/vnd.github+json")
        request.add_header("Authorization", f"Bearer {token}")
        request.add_header("X-GitHub-Api-Version", "2022-11-28")
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                batch = json.loads(response.read().decode())
        except urllib.error.HTTPError as exc:
            fail(f"cannot read the commits of #{number}: HTTP {exc.code}")
        except (urllib.error.URLError, json.JSONDecodeError) as exc:
            fail(f"cannot read the commits of #{number}: {exc}")
        if not isinstance(batch, list):
            fail(f"the commits response for #{number} is not a list")
        commits.extend(batch)
        if len(batch) < 100:
            return commits
    fail(f"the pull request #{number} has more commits than this check reads")


def main(argv: list[str]) -> int:
    """Check every commit and report each offender."""
    if argv[1:2] == ["--self-test"]:
        return self_test()
    repo = os.environ.get("GITHUB_REPOSITORY", "")
    token = os.environ.get("GITHUB_TOKEN", "")
    event_path = os.environ.get("GITHUB_EVENT_PATH", "")
    if not repo or not token or not event_path:
        fail("GITHUB_REPOSITORY, GITHUB_TOKEN and GITHUB_EVENT_PATH must be set")
    try:
        with open(event_path, encoding="utf-8") as handle:
            event = json.load(handle)
    except (OSError, json.JSONDecodeError) as exc:
        fail(f"cannot read the event payload: {exc}")
    number = (event.get("pull_request") or {}).get("number") or (event.get("number"))
    if not isinstance(number, int):
        fail("the event payload names no pull request")
    commits = fetch_commits(
        os.environ.get("GITHUB_API_URL", "https://api.github.com").rstrip("/"),
        repo,
        token,
        number,
    )
    if not commits:
        fail(f"pull request #{number} holds no commits")
    bad = 0
    for commit in commits:
        reason = offender(commit)
        if reason is None:
            continue
        bad += 1
        sha = str(commit.get("sha", ""))
        login = str((commit.get("author") or {}).get("login", "unknown"))
        detail = commit.get("commit") or {}
        email = str((detail.get("author") or {}).get("email", "unknown"))
        print(f"{sha}  {login} <{email}>  {reason}")
    if bad:
        print(f"::error::{bad} of {len(commits)} commits are missing a matching Signed-off-by trailer. Amend the commits with git commit -s and push again.")
        return 1
    print(f"all {len(commits)} commits are signed off")
    return 0


def self_test() -> int:
    """Offline checks over the trailer rule and the skips."""
    cases: list[tuple[str, bool]] = []

    def check(name: str, condition: bool) -> None:
        cases.append((name, condition))

    def commit(login: str, email: str, message: str, kind: str = "User", parents: int = 1) -> dict:
        return {
            "sha": "0" * 40,
            "author": {"login": login, "type": kind},
            "commit": {"author": {"email": email}, "message": message},
            "parents": [{"sha": "p"}] * parents,
        }

    signed = commit("alice", "Alice@Example.com", "fix: a thing\n\nSigned-off-by: Alice <alice@example.com>")
    check("a signed commit passes", offender(signed) is None)
    unsigned = commit("alice", "alice@example.com", "fix: a thing")
    check("an unsigned commit fails", offender(unsigned) is not None)
    check(
        "the failure names the missing trailer",
        "no Signed-off-by" in str(offender(unsigned)),
    )
    wrong = commit("alice", "alice@example.com", "fix: a thing\n\nSigned-off-by: Bob <bob@example.com>")
    check("a wrong-email trailer fails", offender(wrong) is not None)
    check("the wrong-email failure names both", "bob@example.com" in str(offender(wrong)))
    bot = commit("dependabot[bot]", "bot@example.com", "chore: bump", kind="Bot")
    check("a bot author is skipped", offender(bot) is None)
    merge = commit("alice", "alice@example.com", "Merge branch 'x'", parents=2)
    check("a merge commit is skipped", offender(merge) is None)
    check(
        "a trailer with no brackets is not a trailer",
        offender(commit("alice", "alice@example.com", "Signed-off-by: Alice alice@example.com")) is not None,
    )

    failed = [name for name, ok in cases if not ok]
    for name, ok in cases:
        print(f"{'ok  ' if ok else 'FAIL'} {name}")
    print(f"{len(cases) - len(failed)}/{len(cases)} checks passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
