#!/usr/bin/env python3
"""The CLA check and signature recorder, called from `.github/workflows/cla.yml`.

`status` refreshes the sign-request comment and the `CLA` commit status for a
pull request event. `sign` records one signature from an `issue_comment` on a
pull request, then sets the status. The record is `{"contributors": [...]}` in
`signatures/cla.json` on the `cla-signatures` branch, one entry per person.

The write uses the GraphQL `createCommitOnBranch` mutation because its
`expectedHeadOid` rejects a write built on a stale read, so one signature cannot
drop another; a contents API PUT cannot promise that alone. Environment:
`GITHUB_REPOSITORY`, `GITHUB_EVENT_PATH`, `GITHUB_TOKEN` and optionally
`GITHUB_API_URL`. The token is never printed. Exit 1 on any read or write error.
"""

from __future__ import annotations

import base64
import json
import os
import sys
import urllib.error
import urllib.request
from datetime import datetime, timezone

SENTENCE = "I have read the CLA Document and I hereby sign the CLA"
SIGNATURES_BRANCH = "cla-signatures"
SIGNATURES_PATH = "signatures/cla.json"
ALLOWLIST_PATH = "signatures/allowlist.txt"
COMMENT_MARKER = "<!-- cla-sign-request -->"
STATUS_CONTEXT = "CLA"
MAX_ATTEMPTS = 3

COMMENT_BODY = f"""{COMMENT_MARKER}
Thanks for the pull request. One more step before it can merge: sign the [Contributor License Agreement](CLA.md).
Comment exactly this sentence on this pull request, and this check turns green:

> {SENTENCE}

You sign once. Later pull requests pass with no further action."""


def fail(message: str) -> None:
    """Print an annotation and stop the run."""
    print(f"::error::{message}")
    raise SystemExit(1)


class API:
    """A minimal GitHub client. It never prints the token."""

    def __init__(self, token: str, repo: str) -> None:
        self.token = token
        self.repo = repo
        self.root = os.environ.get("GITHUB_API_URL", "https://api.github.com").rstrip("/")

    def _call(self, url: str, method: str, body: dict | None) -> tuple[int, dict]:
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(url, data=data, method=method)
        request.add_header("Accept", "application/vnd.github+json")
        request.add_header("Authorization", f"Bearer {self.token}")
        request.add_header("X-GitHub-Api-Version", "2022-11-28")
        if data is not None:
            request.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                raw = response.read().decode()
                return response.status, json.loads(raw) if raw else {}
        except urllib.error.HTTPError as exc:
            raw = exc.read().decode()
            try:
                payload = json.loads(raw)
            except json.JSONDecodeError:
                payload = {"message": raw[:200]}
            return exc.code, payload

    def rest(self, method: str, path: str, body: dict | None = None) -> tuple[int, object]:
        """Call the REST API with a path such as `/repos/o/r/issues/1/comments`."""
        return self._call(f"{self.root}{path}", method, body)

    def graphql(self, query: str, variables: dict) -> tuple[int, dict]:
        """Call the GraphQL API."""
        return self._call(f"{self.root}/graphql", "POST", {"query": query, "variables": variables})


def parse_signatures(text: str) -> list[dict]:
    """The contributor list, or ValueError when the file is not what it claims."""
    try:
        data = json.loads(text)
    except json.JSONDecodeError as exc:
        raise ValueError(f"not valid JSON: {exc}") from exc
    if not isinstance(data, dict):
        raise ValueError("the top level is not an object")
    contributors = data.get("contributors")
    if not isinstance(contributors, list):
        raise ValueError("no contributors list")
    for entry in contributors:
        if not isinstance(entry, dict) or not isinstance(entry.get("login"), str):
            raise ValueError("a contributor entry has no login string")
    return contributors


def read_signatures(api: API) -> list[dict]:
    """The signatures on the branch, or stop the run when they cannot be read."""
    status, body = api.rest(
        "GET", f"/repos/{api.repo}/contents/{SIGNATURES_PATH}?ref={SIGNATURES_BRANCH}"
    )
    if status != 200 or not isinstance(body, dict):
        fail(f"cannot read {SIGNATURES_PATH} on {SIGNATURES_BRANCH}: HTTP {status}")
    try:
        text = base64.b64decode(body.get("content", "")).decode()
        return parse_signatures(text)
    except (ValueError, UnicodeDecodeError) as exc:
        fail(f"{SIGNATURES_PATH} on {SIGNATURES_BRANCH} is malformed: {exc}")


def read_allowlist(api: API, default_branch: str) -> set[str]:
    """The logins that never need to sign. A missing file is an empty list."""
    status, body = api.rest(
        "GET", f"/repos/{api.repo}/contents/{ALLOWLIST_PATH}?ref={default_branch}"
    )
    if status == 404:
        return set()
    if status != 200 or not isinstance(body, dict):
        fail(f"cannot read {ALLOWLIST_PATH}: HTTP {status}")
    try:
        text = base64.b64decode(body.get("content", "")).decode().lower()
    except (ValueError, UnicodeDecodeError) as exc:
        fail(f"{ALLOWLIST_PATH} is not UTF-8 text: {exc}")
    logins = set()
    for line in text.splitlines():
        entry = line.split("#", 1)[0].strip()
        if entry:
            logins.add(entry)
    return logins


def is_bot(user: dict) -> bool:
    """True for a GitHub App account: it can never sign."""
    login = str(user.get("login", ""))
    return user.get("type") == "Bot" or login.endswith("[bot]")


def has_signed(login: str, contributors: list[dict], allowlist: set[str]) -> bool:
    """True when this login needs no further action."""
    if login.lower() in allowlist:
        return True
    return any(entry["login"].lower() == login.lower() for entry in contributors)


def set_status(api: API, sha: str, state: str, description: str, url: str) -> None:
    """Write the `CLA` commit status. The check name is the context."""
    body = {
        "state": state,
        "context": STATUS_CONTEXT,
        "description": description[:140],
        "target_url": url,
    }
    status, _ = api.rest("POST", f"/repos/{api.repo}/statuses/{sha}", body)
    if status not in (200, 201):
        fail(f"cannot set the {STATUS_CONTEXT} status on {sha}: HTTP {status}")


def upsert_comment(api: API, number: int, body: str) -> None:
    """Post the sign-request comment once, then keep its text current."""
    status, comments = api.rest(
        "GET", f"/repos/{api.repo}/issues/{number}/comments?per_page=100"
    )
    if status != 200 or not isinstance(comments, list):
        fail(f"cannot list comments on #{number}: HTTP {status}")
    for comment in comments:
        if COMMENT_MARKER not in str(comment.get("body", "")):
            continue
        if comment.get("body") == body:
            return
        status, _ = api.rest(
            "PATCH", f"/repos/{api.repo}/issues/comments/{comment['id']}", {"body": body}
        )
        if status != 200:
            fail(f"cannot update the sign-request comment on #{number}: HTTP {status}")
        return
    status, _ = api.rest(
        "POST", f"/repos/{api.repo}/issues/{number}/comments", {"body": body}
    )
    if status != 201:
        fail(f"cannot post the sign-request comment on #{number}: HTTP {status}")


def signature_record(api: API, login: str, sha: str) -> dict:
    """One record, taking the name and email from the head commit's author."""
    status, body = api.rest("GET", f"/repos/{api.repo}/commits/{sha}")
    name = email = ""
    if status == 200 and isinstance(body, dict):
        author = (body.get("commit") or {}).get("author") or {}
        name = str(author.get("name", ""))
        email = str(author.get("email", ""))
    record = {
        "login": login,
        "name": name,
        "email": email,
        "date": datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z"),
        "commit": sha,
    }
    return record


def append_signature(api: API, record: dict) -> bool:
    """Append one record with a conflict-guarded commit. False when already there."""
    query = """
    mutation($input: CreateCommitOnBranchInput!) {
      createCommitOnBranch(input: $input) { commit { oid } }
    }
    """
    for attempt in range(1, MAX_ATTEMPTS + 1):
        status, ref = api.rest("GET", f"/repos/{api.repo}/git/ref/heads/{SIGNATURES_BRANCH}")
        if status != 200 or not isinstance(ref, dict):
            fail(f"cannot read the {SIGNATURES_BRANCH} branch: HTTP {status}")
        head = ref["object"]["sha"]
        status, blob = api.rest(
            "GET", f"/repos/{api.repo}/contents/{SIGNATURES_PATH}?ref={SIGNATURES_BRANCH}"
        )
        if status != 200 or not isinstance(blob, dict):
            fail(f"cannot read {SIGNATURES_PATH} on {SIGNATURES_BRANCH}: HTTP {status}")
        try:
            text = base64.b64decode(blob.get("content", "")).decode()
            contributors = parse_signatures(text)
        except (ValueError, UnicodeDecodeError) as exc:
            fail(f"{SIGNATURES_PATH} on {SIGNATURES_BRANCH} is malformed: {exc}")
        if has_signed(record["login"], contributors, set()):
            return False
        contributors.append(record)
        document = {"contributors": contributors}
        encoded = base64.b64encode(
            (json.dumps(document, indent=2, ensure_ascii=False) + "\n").encode()
        ).decode()
        variables = {
            "input": {
                "branch": {
                    "repositoryNameWithOwner": api.repo,
                    "branchName": SIGNATURES_BRANCH,
                },
                "message": {
                    "headline": f"cla: record the signature of {record['login']}",
                    "body": "Recorded by the CLA workflow.",
                },
                "fileChanges": {
                    "additions": [{"path": SIGNATURES_PATH, "contents": encoded}]
                },
                "expectedHeadOid": head,
            }
        }
        status, result = api.graphql(query, variables)
        if status == 200 and not result.get("errors"):
            return True
        if attempt == MAX_ATTEMPTS:
            fail(f"cannot commit the signature after {MAX_ATTEMPTS} attempts: {result}")
        print(f"commit attempt {attempt} failed on a conflict; retrying")
    return False


def load_event() -> dict:
    """The workflow event payload."""
    path = os.environ.get("GITHUB_EVENT_PATH", "")
    try:
        with open(path, encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, json.JSONDecodeError) as exc:
        fail(f"cannot read the event payload: {exc}")
    return {}


def repo_and_api() -> tuple[dict, API]:
    """The event and a client for the repository the event names."""
    event = load_event()
    repo = os.environ.get("GITHUB_REPOSITORY", "")
    token = os.environ.get("GITHUB_TOKEN", "")
    if not repo or not token:
        fail("GITHUB_REPOSITORY and GITHUB_TOKEN must be set")
    return event, API(token, repo)


def status_command() -> int:
    """Refresh the comment and the status for a pull request event."""
    event, api = repo_and_api()
    pull = event.get("pull_request") or {}
    number = pull.get("number")
    login = str((pull.get("user") or {}).get("login", ""))
    sha = str((pull.get("head") or {}).get("sha", ""))
    url = str(pull.get("html_url", ""))
    if not number or not login or not sha:
        fail("the pull_request payload is missing number, author or head sha")
    contributors = read_signatures(api)
    default_branch = str((event.get("repository") or {}).get("default_branch", "master"))
    allow = read_allowlist(api, default_branch)
    if has_signed(login, contributors, allow):
        set_status(api, sha, "success", "The Contributor License Agreement is signed.", url)
        return 0
    upsert_comment(api, int(number), COMMENT_BODY)
    set_status(
        api,
        sha,
        "pending",
        "Sign the CLA with one comment on this pull request.",
        url,
    )
    return 0


def sign_command() -> int:
    """Record a signature from a qualifying comment, then update the status."""
    event, api = repo_and_api()
    issue = event.get("issue") or {}
    comment = event.get("comment") or {}
    author = comment.get("user") or {}
    if not issue.get("pull_request"):
        return 0
    if is_bot(author):
        print("ignoring a comment by a bot")
        return 0
    number = int(issue["number"])
    login = str(author.get("login", ""))
    status, pull = api.rest("GET", f"/repos/{api.repo}/pulls/{number}")
    if status != 200 or not isinstance(pull, dict):
        fail(f"cannot read pull request #{number}: HTTP {status}")
    owner = str((pull.get("user") or {}).get("login", ""))
    sha = str((pull.get("head") or {}).get("sha", ""))
    url = str(pull.get("html_url", ""))
    if login.lower() != owner.lower():
        print("ignoring a comment that is not from the pull request author")
        return 0
    if str(comment.get("body", "")).strip().lower() != SENTENCE.lower():
        return 0
    contributors = read_signatures(api)
    default_branch = str((event.get("repository") or {}).get("default_branch", "master"))
    allow = read_allowlist(api, default_branch)
    if not has_signed(login, contributors, allow):
        record = signature_record(api, login, sha)
        if append_signature(api, record):
            print(f"recorded the signature of {login}")
        else:
            print(f"{login} had already signed")
    set_status(api, sha, "success", "The Contributor License Agreement is signed.", url)
    return 0


def self_test() -> int:
    """Offline checks over the parsing, the duplicate rule and the sign path."""

    class FakeAPI(API):
        def __init__(self, signatures: str) -> None:
            super().__init__("token-not-printed", "owner/name")
            self.signatures = signatures
            self.commits: list[dict] = []
            self.statuses: list[dict] = []
            self.comments: list[dict] = []

        def rest(self, method, path, body=None):
            if path.startswith("/repos/owner/name/git/ref/heads/"):
                return 200, {"object": {"sha": "head1"}}
            if "/contents/signatures/cla.json" in path:
                content = base64.b64encode(self.signatures.encode()).decode()
                return 200, {"content": content}
            if "/contents/signatures/allowlist.txt" in path:
                return 404, {}
            if path.endswith("/commits/abc123"):
                return 200, {"commit": {"author": {"name": "A", "email": "a@example.com"}}}
            if "/statuses/" in path:
                self.statuses.append(body)
                return 201, {}
            if method == "GET" and path.endswith("/comments?per_page=100"):
                return 200, self.comments
            if method == "POST" and path.endswith("/comments"):
                self.comments.append({"id": 7, "body": body["body"]})
                return 201, {}
            return 404, {}

        def graphql(self, query, variables):
            self.commits.append(variables["input"])
            encoded = variables["input"]["fileChanges"]["additions"][0]["contents"]
            self.signatures = base64.b64decode(encoded).decode()
            return 200, {"data": {"createCommitOnBranch": {"commit": {"oid": "new"}}}}

    cases: list[tuple[str, bool]] = []

    def check(name: str, condition: bool) -> None:
        cases.append((name, condition))

    good = '{"contributors": [{"login": "alice", "name": "A", "email": "a@x", "date": "d", "commit": "c"}]}'
    try:
        check("a well-formed file parses", len(parse_signatures(good)) == 1)
    except ValueError:
        check("a well-formed file parses", False)
    for name, text in (
        ("broken JSON is malformed", "{not json"),
        ("a missing list is malformed", '{"contributors": {}}'),
        ("an entry without a login is malformed", '{"contributors": [{"name": "A"}]}'),
    ):
        try:
            parse_signatures(text)
            check(name, False)
        except ValueError:
            check(name, True)

    check("a signed login passes", has_signed("alice", parse_signatures(good), set()))
    check("an unsigned login is pending", not has_signed("bob", parse_signatures(good), set()))
    check("the allowlist passes a login", has_signed("dependabot[bot]", [], {"dependabot[bot]"}))
    check("a login comparison ignores case", has_signed("ALICE", parse_signatures(good), set()))
    check("a bot comment is ignored", is_bot({"login": "x[bot]", "type": "Bot"}))
    check("a person is not a bot", not is_bot({"login": "alice", "type": "User"}))
    check(
        "the sentence match is trimmed and case-insensitive",
        SENTENCE.lower() == "  i have read the cla document and i hereby sign the cla  ".strip().lower(),
    )

    api = FakeAPI(good)
    record = {"login": "bob", "name": "B", "email": "b@x", "date": "d", "commit": "c"}
    check("a new signature is appended", append_signature(api, record))
    check("a second signature is not appended", not append_signature(api, record))
    entries = parse_signatures(api.signatures)
    check("the file holds one entry per person", [e["login"] for e in entries] == ["alice", "bob"])

    api = FakeAPI("{not json")
    try:
        append_signature(api, record)
        check("a malformed file stops the write", False)
    except SystemExit:
        check("a malformed file stops the write", True)

    failed = [name for name, ok in cases if not ok]
    for name, ok in cases:
        print(f"{'ok  ' if ok else 'FAIL'} {name}")
    print(f"{len(cases) - len(failed)}/{len(cases)} checks passed")
    return 1 if failed else 0


def main(argv: list[str]) -> int:
    """Dispatch one subcommand."""
    if argv[1:2] == ["--self-test"]:
        return self_test()
    if argv[1:2] == ["status"]:
        return status_command()
    if argv[1:2] == ["sign"]:
        return sign_command()
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
