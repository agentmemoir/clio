"""Shared fixtures for the pre-commit runner tests.

A throwaway git repository with a configuration of its own, a way to run the real
runner in it, and a base case that builds one per test. The tests live in
`test_precommit.py`, `test_precommit_scopes.py`, `test_precommit_shim.py`, and
`test_precommit_all_files.py`.
"""

from __future__ import annotations

import os
import subprocess
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from exec_tempdir import exec_tempdir  # noqa: E402

RUNNER = Path(__file__).resolve().parent / "precommit.py"
SHIM = Path(__file__).resolve().parent / "git-hooks" / "pre-commit"
MERGE_SHIM = Path(__file__).resolve().parent / "git-hooks" / "pre-merge-commit"

#: A hook that fails when any file it is given contains this marker. Stands in
#: for a check that reads the change, which is what a commit hook is for.
#:
#: The marker is assembled at run time from two halves, because the hook is
#: itself a staged file: spelled out in one piece, the hook would reject its own
#: source and every test that installs it would fail for the wrong reason. That
#: is not a hypothetical, it is what the first version of this string did.
LEAK_HOOK = (
    'm="LE""AK"\n'
    'grep -l "$m" "$@" >/dev/null 2>&1 && { echo "found a leak"; exit 1; }\n'
    "exit 0"
)
#: What a file has to contain for the hook above to reject it.
LEAK_MARKER = "LEAK"

#: The git configuration a fixture repository answers for itself.
#:
#: Git reads three configurations, and its own is read last, so a value written
#: here beats anything the machine running the suite has set. That is the whole
#: point of this table: a throwaway repository built to be independent of the
#: machine is not independent while a commit in it can still ask the machine who
#: it is. One place, applied once when the repository is made, so every `git`
#: call afterwards -- the suite's own, the runner's, and the ones git makes while
#: running a hook -- is covered without each having to remember a flag.
#:
#: Every key is here because a host that set it was measured failing this suite.
#: With no identity, every commit is refused. `commit.gpgsign` refuses them too,
#: on any machine without a signing key, and a `-c` flag naming the author does
#: not help: `-c` overrides the keys it names and nothing else. A `core.autocrlf`
#: rewrites a freshly checked-out hook's shebang to `#!/bin/sh\r`, which the
#: kernel cannot execute. A `core.fsmonitor` that is not a working daemon fails
#: the commit outright. `core.hooksPath` is not here because it needs a path from
#: this repository rather than a constant; it is written by `configure`.
FIXTURE_CONFIG: dict[str, str] = {
    "user.name": "t",
    "user.email": "t@e",
    "commit.gpgsign": "false",
    "core.autocrlf": "false",
    "core.fsmonitor": "false",
}


def configure(root: Path) -> None:
    """Give the repository at `root` a git configuration of its own.

    `core.hooksPath` gets an absolute path on purpose. A host may point it at a
    directory of its own, and then git looks there instead of here, finds no
    `pre-commit`, and commits anyway without saying so -- the one outcome this
    suite exists to refuse. A relative path would not do: git resolves one
    against the top of the working tree, and in a linked worktree that tree's
    `.git` is a file, not a directory. The absolute path of the main checkout's
    hook directory is what git reaches on its own anyway, because `.git/hooks` is
    shared by every worktree of a repository.
    """
    for key, value in FIXTURE_CONFIG.items():
        subprocess.run(  # noqa: S603
            ["git", "config", "--local", key, value], cwd=root, check=True  # noqa: S607
        )
    subprocess.run(  # noqa: S603
        ["git", "config", "--local", "core.hooksPath", str(root / ".git" / "hooks")],
        cwd=root,
        check=True,
    )


def build_repo(root: Path) -> None:
    """A minimal repository with one tracked text file and one tracked binary.

    Configured before the first commit, so the identity that commit needs comes
    from the repository rather than from a flag on the call that happens to make
    it.
    """
    subprocess.run(["git", "init", "-q"], cwd=root, check=True)  # noqa: S603, S607
    configure(root)
    (root / "a.rs").write_text("fn main() {}\n", encoding="utf-8")
    (root / "b.md").write_text("# doc\n", encoding="utf-8")
    (root / "c.bin").write_bytes(b"\x00\x01\x02\xff\xfe")
    (root / "sub").mkdir()
    (root / "sub" / "d.rs").write_text("// nested\n", encoding="utf-8")
    subprocess.run(  # noqa: S603
        ["git", "add", "-A"], cwd=root, check=True  # noqa: S607
    )
    subprocess.run(["git", "commit", "-qm", "init"], cwd=root, check=True)  # noqa: S603, S607


def run(root: Path, *args: str, env: dict[str, str] | None = None) -> subprocess.CompletedProcess:
    """Run the real runner inside `root`."""
    environment = dict(os.environ)
    environment.pop("CLIO_PRECOMMIT_HOOKS", None)
    if env:
        environment.update(env)
    return subprocess.run(  # noqa: S603
        [sys.executable, str(RUNNER), *args],
        cwd=root,
        capture_output=True,
        text=True,
        env=environment,
        check=False,
    )


def write_hook(path: Path, body: str = "exit 0", executable: bool = True) -> Path:
    """Write an executable-free-by-default shell hook."""
    path.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
    if executable:
        path.chmod(0o755)
    return path


class RepoCase(unittest.TestCase):
    """A fresh repository per test, removed afterwards."""

    def setUp(self) -> None:
        self._tmp = exec_tempdir()
        self.root = Path(self._tmp.name)
        # Registered before the repository is built, because a build that fails
        # ends the test there. Cleaning up afterwards would leave the directory
        # behind, and the run that failed for one reason would then also litter
        # the machine it failed on.
        self.addCleanup(self._tmp.cleanup)
        build_repo(self.root)
