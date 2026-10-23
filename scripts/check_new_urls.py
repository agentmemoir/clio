"""Fails when a URL host in a list is not covered by the domain allow-list.

Usage:
  python3 scripts/check_new_urls.py <urls-file> <allowlist-file>

The URL list holds one URL per line, as produced by the workflow's diff step.
Each URL is reduced to a host: the scheme, userinfo, port, path, query and
fragment are dropped and the host is lowercased. A URL with an empty host is
skipped, because it gives nothing to compare.

The allow-list holds one domain per line. Blank lines and lines that start with
`#` are skipped and case is ignored on both sides. An entry covers a host when
the two are equal or when the host ends with a dot plus the entry, so
`example.com` covers `v2.example.com` but `bar.example.com` does not cover
`notbar.example.com`. A missing or empty list is a hard error.

Each uncovered domain is printed as `domain  url`. The exit code is 1 when at
least one domain is uncovered and 0 when every domain is covered.

Exit codes:
  0  every host is covered
  1  at least one host is uncovered, or the allow-list could not be read
  2  usage error
"""

from __future__ import annotations

import sys


def domain(url: str) -> str:
    """The lowercase host of a URL, or an empty string when it holds none."""
    s = url.strip()
    if "://" in s:
        s = s.split("://", 1)[1]
    s = s.split("/", 1)[0].split("?", 1)[0].split("#", 1)[0]
    s = s.rsplit("@", 1)[-1]
    if s.startswith("["):
        host = s.split("]", 1)[0] + "]"
    else:
        host = s.split(":", 1)[0]
    return host.lower()


def load_allowlist(path: str) -> set[str] | None:
    """The domains in the allow-list, or None when the file cannot be read."""
    try:
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
    except OSError as exc:
        print(f"::error::cannot read {path}: {exc}")
        return None
    allow = set()
    for line in text.splitlines():
        entry = line.strip().lower()
        if entry and not entry.startswith("#"):
            allow.add(entry)
    return allow


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print("usage: check_new_urls.py <urls-file> <allowlist-file>", file=sys.stderr)
        return 2
    pairs_path, list_path = argv
    allow = load_allowlist(list_path)
    if allow is None:
        return 1
    if not allow:
        print(f"::error::{list_path} has no entries")
        return 1
    offenders = []
    with open(pairs_path, encoding="utf-8") as handle:
        for line in handle:
            url = line.strip()
            if not url:
                continue
            host = domain(url)
            if not host:
                continue
            if not any(host == e or host.endswith("." + e) for e in allow):
                offenders.append((host, url))
    if offenders:
        print(f"::error::URL domains not covered by {list_path}:")
        for host, url in offenders:
            print(f"{host}  {url}")
        return 1
    print(f"all URL domains are covered by {list_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
