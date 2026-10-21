#!/usr/bin/env python3
"""Tests for scripts/gate-install.sh: the shim it links and the verdict it gives.

The shared helpers live in scripts/gate_install_support.py, which says what the installer checks, why these tests exist, and how to run this suite. The cases that ask a shell which profile to read live in scripts/test_gate_install_profiles.py.
"""

from __future__ import annotations

import os
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from gate_install_support import (  # noqa: E402
    BASH,
    SYSTEM_PATH,
    install_tree,
    make_controlled_path,
    make_timeout_bin,
    run_installer,
    stub_tool,
)


class GateInstallTests(unittest.TestCase):
    def setUp(self) -> None:
        self.base, self.dest, self.earlier, self.home = install_tree(self)

    def test_a_shadowed_shim_reports_not_active_and_exits_non_zero(self) -> None:
        # `earlier` precedes `dest`, so both names resolve to the stand-ins and
        # the shim the installer just linked can never run.
        stub_tool(self.earlier, "cargo")
        stub_tool(self.earlier, "make")
        result = run_installer(
            self.dest,
            os.pathsep.join(
                (
                    str(make_timeout_bin(self.base)),
                    str(self.earlier),
                    SYSTEM_PATH,
                    str(self.dest),
                )
            ),
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
            os.pathsep.join(
                (
                    str(self.dest),
                    str(make_timeout_bin(self.base)),
                    SYSTEM_PATH,
                    str(self.earlier),
                )
            ),
            home=self.home,
        )
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertIn("shim is active", output)

    def test_a_sibling_directory_under_dest_is_not_the_shim(self) -> None:
        # The old prefix test accepted any executable beneath `dest`, so a
        # different program at `dest/tools/make` read as the shim. Only a
        # symlink whose target is the installed shim counts.
        sibling = self.dest / "tools"
        sibling.mkdir()
        for name in ("cargo", "make"):
            stub = sibling / name
            stub.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            stub.chmod(0o755)
        result = run_installer(
            self.dest,
            os.pathsep.join(
                (
                    str(make_timeout_bin(self.base)),
                    str(sibling),
                    SYSTEM_PATH,
                    str(self.dest),
                )
            ),
            home=self.home,
        )
        output = result.stdout + result.stderr
        self.assertNotEqual(result.returncode, 0, output)
        self.assertIn("NOT ACTIVE", output)

    def test_an_unset_shell_does_not_break_the_install(self) -> None:
        # SHELL may be absent. The installer must still run. On macOS the
        # launching `sh` refills SHELL from the passwd database, so which shell
        # is chosen is not observable here; the install must simply succeed.
        result = run_installer(
            self.dest,
            os.pathsep.join(
                (
                    str(self.dest),
                    str(make_timeout_bin(self.base)),
                    SYSTEM_PATH,
                    str(self.earlier),
                )
            ),
            home=self.home,
            drop_shell=True,
        )
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertIn("shim is active", output)

    def test_an_unresolvable_login_shell_falls_back_to_bin_sh(self) -> None:
        # Force the account lookup to fail: `id`, `getent`, and `dscl` are
        # stubs that exit 1, and SHELL is unusable. The last resort is /bin/sh;
        # no profile on its PATH adds `dest`, so the install must report
        # NOT ACTIVE and exit non-zero instead of reading the shim as active.
        fakebin = self.base / "noid"
        fakebin.mkdir()
        for tool in ("id", "getent", "dscl"):
            stub = fakebin / tool
            stub.write_text("#!/bin/sh\nexit 1\n", encoding="utf-8")
            stub.chmod(0o755)
        result = run_installer(
            self.dest,
            os.pathsep.join(
                (str(make_timeout_bin(self.base)), str(fakebin), SYSTEM_PATH)
            ),
            home=self.home,
            shell="/no/such/shell",
        )
        output = result.stdout + result.stderr
        self.assertNotEqual(result.returncode, 0, output)
        self.assertIn("NOT ACTIVE", output)

    def test_a_login_hit_with_an_interactive_shadow_names_the_divergence(self) -> None:
        # A login profile can add `dest` while the interactive profile puts a
        # real tool first. The login shell counts, so the install stays active,
        # but the shell the user runs gates in does not see the shim; that must
        # be named rather than hidden behind a bare "active in a login shell".
        if BASH is None:
            self.skipTest("no bash to act as the login and interactive shell")
        stub_tool(self.earlier, "cargo")
        stub_tool(self.earlier, "make")
        (self.home / ".profile").write_text(
            f'export PATH="{self.dest}:$PATH"\n', encoding="utf-8"
        )
        (self.home / ".bashrc").write_text(
            f'export PATH="{self.earlier}:$PATH"\n', encoding="utf-8"
        )
        result = run_installer(
            self.dest, make_controlled_path(self.base), home=self.home, shell=BASH
        )
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertIn("not active in the interactive shell", output)

    def test_a_regular_file_at_dest_make_is_reported_and_fails(self) -> None:
        # A foreign regular file placed directly at dest/<name>, not a symlink,
        # must not be clobbered or read as the shim. The installer reports the
        # existing file and exits non-zero.
        foreign = self.dest / "make"
        content = "#!/bin/sh\nexit 0\n"
        foreign.write_text(content, encoding="utf-8")
        foreign.chmod(0o755)
        result = run_installer(
            self.dest,
            os.pathsep.join((str(self.dest), SYSTEM_PATH, str(self.earlier))),
            home=self.home,
        )
        output = result.stdout + result.stderr
        self.assertNotEqual(result.returncode, 0, output)
        self.assertEqual(foreign.read_text(encoding="utf-8"), content)
        self.assertIn("exists and is not a symlink", output)


if __name__ == "__main__":
    unittest.main()
