#!/usr/bin/env python3
"""Tests for scripts/gate-install.sh.

Why these exist
---------------
The installer links `cargo` and `make` to the gate shim in a directory the
caller later puts on PATH. A symlink in a directory that comes *after* the real
tool never runs, so the shim can be installed and dead. The installer used to
test only that its directory appeared somewhere on PATH and printed a success
line either way, so a dead shim read as a clean install and every later run
quoted a verdict no live run had refreshed.

These tests pin the correction: the installer resolves each name, reports
NOT ACTIVE and exits non-zero when a name lands outside its directory, and
exits zero when both names resolve to the shim. Each test builds a controlled
PATH, so the answer does not depend on what the host has installed.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from exec_tempdir import exec_tempdir  # noqa: E402

INSTALLER = Path(__file__).resolve().parent / "gate-install.sh"
SH = shutil.which("sh") or "/bin/sh"
BASH = shutil.which("bash")

#: The directories that hold the external commands the installer itself calls
#: (`dirname`, `mkdir`, `ln`, `chmod`, `readlink`). They are kept on PATH so the
#: controlled PATH does not also hide the tools the script needs to run.
SYSTEM_PATH = os.pathsep.join(("/usr/bin", "/bin"))


def run_installer(
    dest: Path,
    path: str,
    home: Path | None = None,
    shell: str | None = None,
) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env["PATH"] = path
    if home is not None:
        env["HOME"] = str(home)
    if shell is not None:
        env["SHELL"] = shell
    return subprocess.run(
        [SH, str(INSTALLER), "--dest", str(dest)],
        capture_output=True,
        text=True,
        env=env,
        check=False,
    )


class GateInstallTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = exec_tempdir(purpose="the gate shim installer test")
        self.addCleanup(self._tmp.cleanup)
        root = Path(self._tmp.name)
        self.dest = root / "dest"
        self.earlier = root / "earlier"
        self.home = root / "home"
        self.dest.mkdir()
        self.earlier.mkdir()
        self.home.mkdir()

    def stub_tool(self, name: str) -> None:
        """A stand-in for a real tool in the directory that comes first on PATH."""
        path = self.earlier / name
        path.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        path.chmod(0o755)

    def test_a_shadowed_shim_reports_not_active_and_exits_non_zero(self) -> None:
        # `earlier` precedes `dest`, so both names resolve to the stand-ins and
        # the shim the installer just linked can never run.
        self.stub_tool("cargo")
        self.stub_tool("make")
        result = run_installer(
            self.dest,
            os.pathsep.join((str(self.earlier), SYSTEM_PATH, str(self.dest))),
            home=self.home,
        )
        output = result.stdout + result.stderr
        self.assertNotEqual(result.returncode, 0, output)
        self.assertIn("NOT ACTIVE", output)
        self.assertIn("make", output)
        # The old message reported success for this exact state.
        self.assertNotIn("PATH already contains", output)

    def test_a_shim_first_on_path_is_active_and_exits_zero(self) -> None:
        result = run_installer(
            self.dest,
            os.pathsep.join((str(self.dest), SYSTEM_PATH, str(self.earlier))),
            home=self.home,
        )
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertIn("shim is active", output)

    def test_a_sibling_directory_under_dest_is_not_the_shim(self) -> None:
        # The old prefix test accepted any executable beneath `dest`, so a
        # different program at `dest/tools/make` read as the shim. Only the
        # exact link the installer made, or the shim itself, is the shim.
        sibling = self.dest / "tools"
        sibling.mkdir()
        for name in ("cargo", "make"):
            stub = sibling / name
            stub.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            stub.chmod(0o755)
        result = run_installer(
            self.dest,
            os.pathsep.join((str(sibling), SYSTEM_PATH, str(self.dest))),
            home=self.home,
        )
        output = result.stdout + result.stderr
        self.assertNotEqual(result.returncode, 0, output)
        self.assertIn("NOT ACTIVE", output)

    def test_a_login_profile_that_adds_dest_exits_zero(self) -> None:
        # A plain `ssh` shell reads no profile, so its PATH misses `dest` even on
        # a machine set up correctly. The installer must ask a login shell
        # before it fails the install.
        if BASH is None:
            self.skipTest("no bash to act as the login shell")
        (self.home / ".profile").write_text(
            f'export PATH="{self.dest}:$PATH"\n', encoding="utf-8"
        )
        result = run_installer(self.dest, SYSTEM_PATH, home=self.home, shell=BASH)
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertIn("shim is active", output)

    def test_an_interactive_profile_that_adds_dest_exits_zero(self) -> None:
        # The install writes the PATH line to the interactive profile
        # (`~/.bashrc` or `~/.zshrc`), which a login shell does not read. An
        # interactive shell is the one that sees it, so the check must ask it.
        if BASH is None:
            self.skipTest("no bash to act as the interactive shell")
        (self.home / ".bashrc").write_text(
            f'export PATH="{self.dest}:$PATH"\n', encoding="utf-8"
        )
        result = run_installer(self.dest, SYSTEM_PATH, home=self.home, shell=BASH)
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertIn("shim is active", output)


if __name__ == "__main__":
    unittest.main()
