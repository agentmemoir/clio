#!/usr/bin/env python3
"""Tests for scripts/gate-install.sh.

The shared helpers live in scripts/gate_install_support.py, which says what the
installer checks, why these tests exist, and how to run this suite.
"""

from __future__ import annotations

import os
import shutil
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from gate_install_support import (  # noqa: E402
    BASH,
    SYSTEM_PATH,
    ZSH,
    account_login_shell,
    exec_tempdir,
    make_controlled_path,
    make_timeout_bin,
    run_installer,
    stub_tool,
)


class GateInstallTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = exec_tempdir(purpose="the gate shim installer test")
        self.addCleanup(self._tmp.cleanup)
        self.base = Path(self._tmp.name)
        self.dest = self.base / "dest"
        self.earlier = self.base / "earlier"
        self.home = self.base / "home"
        self.dest.mkdir()
        self.earlier.mkdir()
        self.home.mkdir()

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

    def test_a_login_profile_that_adds_dest_exits_zero(self) -> None:
        # A plain `ssh` shell reads no profile, so its PATH misses `dest` even on
        # a machine set up correctly. The installer must ask a login shell
        # before it fails the install.
        if BASH is None:
            self.skipTest("no bash to act as the login shell")
        (self.home / ".profile").write_text(
            f'export PATH="{self.dest}:$PATH"\n', encoding="utf-8"
        )
        result = run_installer(
            self.dest, make_controlled_path(self.base), home=self.home, shell=BASH
        )
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
        result = run_installer(
            self.dest, make_controlled_path(self.base), home=self.home, shell=BASH
        )
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertIn("shim is active", output)

    def test_a_profile_that_prints_does_not_hide_the_shim(self) -> None:
        # A profile often prints to stdout. The probe must find the shim among
        # the printed lines, or a working machine reads as inactive.
        if BASH is None:
            self.skipTest("no bash to act as the interactive shell")
        (self.home / ".bashrc").write_text(
            f'echo "welcome back"\nexport PATH="{self.dest}:$PATH"\n',
            encoding="utf-8",
        )
        result = run_installer(
            self.dest, make_controlled_path(self.base), home=self.home, shell=BASH
        )
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertIn("shim is active", output)

    def test_a_zsh_login_profile_that_adds_dest_exits_zero(self) -> None:
        # The installer is used on macOS, where the login profile is
        # ~/.zprofile. A zsh login shell is a shell that runs gates too.
        if ZSH is None:
            self.skipTest("no zsh to act as the login shell")
        (self.home / ".zprofile").write_text(
            f'export PATH="{self.dest}:$PATH"\n', encoding="utf-8"
        )
        result = run_installer(
            self.dest, make_controlled_path(self.base), home=self.home, shell=ZSH
        )
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertIn("shim is active", output)

    def test_a_zsh_interactive_profile_that_adds_dest_exits_zero(self) -> None:
        # macOS writes the PATH line to ~/.zshrc, which a login shell does not
        # read, so the interactive zsh shell is the one that finds it.
        if ZSH is None:
            self.skipTest("no zsh to act as the interactive shell")
        (self.home / ".zshrc").write_text(
            f'export PATH="{self.dest}:$PATH"\n', encoding="utf-8"
        )
        result = run_installer(
            self.dest, make_controlled_path(self.base), home=self.home, shell=ZSH
        )
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertIn("shim is active", output)

    def test_a_probe_timeout_is_not_read_as_inactive(self) -> None:
        # A profile slower than the 5s probe budget is killed by `timeout`,
        # which exits 124. That means "could not probe", not "the shim is
        # inactive", so it must not fail an otherwise correct install (#213).
        # A stand-in `timeout` that returns 124 pins this without a real sleep.
        fakebin = self.base / "fakebin"
        fakebin.mkdir()
        fake = fakebin / "timeout"
        fake.write_text("#!/bin/sh\nexit 124\n", encoding="utf-8")
        fake.chmod(0o755)
        stub_tool(self.earlier, "cargo")
        stub_tool(self.earlier, "make")
        result = run_installer(
            self.dest,
            os.pathsep.join(
                (str(fakebin), SYSTEM_PATH, str(self.earlier), str(self.dest))
            ),
            home=self.home,
        )
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertNotIn("NOT ACTIVE", output)
        self.assertIn("did not finish within the 5s probe budget", output)

    def test_a_gtimeout_alone_still_checks_the_profile(self) -> None:
        # macOS has no `timeout`; GNU coreutils installs the same tool as
        # `gtimeout`. The installer must find it there, or every macOS host
        # reads the shim as unverifiable and the install fails. The PATH here
        # holds `gtimeout` and the tools the installer itself calls, and no
        # `timeout`, so the test proves the gtimeout branch and not the
        # ordinary one.
        if BASH is None:
            self.skipTest("no bash to act as the login shell")
        toolbin = self.base / "gtoolbin"
        toolbin.mkdir()
        for name in ("chmod", "dirname", "ln", "mkdir", "readlink"):
            source = shutil.which(name, path=SYSTEM_PATH)
            self.assertIsNotNone(source)
            os.symlink(source, toolbin / name)
        (self.home / ".profile").write_text(
            f'export PATH="{self.dest}:$PATH"\n', encoding="utf-8"
        )
        result = run_installer(
            self.dest,
            os.pathsep.join(
                (str(make_timeout_bin(self.base, "gtimeout")), str(toolbin))
            ),
            home=self.home,
            shell=BASH,
        )
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertIn("shim is active", output)

    def test_a_probe_without_timeout_is_not_run(self) -> None:
        # There is no portable profile watchdog. If `timeout` is unavailable,
        # the installer must leave a blocking profile unprobed.
        toolbin = self.base / "toolbin"
        toolbin.mkdir()
        for name in ("chmod", "dirname", "ln", "mkdir", "readlink"):
            source = shutil.which(name, path=SYSTEM_PATH)
            self.assertIsNotNone(source)
            os.symlink(source, toolbin / name)
        blocking_shell = self.base / "blocking-shell"
        blocking_shell.write_text("#!/bin/sh\n/bin/sleep 60\n", encoding="utf-8")
        blocking_shell.chmod(0o755)
        stub_tool(self.earlier, "cargo")
        stub_tool(self.earlier, "make")
        result = run_installer(
            self.dest,
            os.pathsep.join((str(toolbin), str(self.earlier), str(self.dest))),
            home=self.home,
            shell=str(blocking_shell),
            timeout=1,
        )
        output = result.stdout + result.stderr
        self.assertNotEqual(result.returncode, 0, output)
        self.assertIn("profile verification is unavailable", output)

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

    def test_a_bogus_shell_resolves_the_account_login_shell(self) -> None:
        # A SHELL path that is not executable must not stop the check. The
        # installer falls back to the account's login shell and reads the
        # profile that shell runs at login.
        shell = account_login_shell()
        if shell is None:
            self.skipTest("could not determine the account login shell")
        name = Path(shell).name
        if not os.access(shell, os.X_OK):
            self.skipTest(f"login shell {shell!r} is not executable")
        if name == "zsh":
            profile = ".zprofile"
        elif name in ("bash", "sh", "dash", "ksh"):
            profile = ".profile"
        else:
            self.skipTest(f"login shell {shell!r} reads an unknown profile")
        (self.home / profile).write_text(
            f'export PATH="{self.dest}:$PATH"\n', encoding="utf-8"
        )
        result = run_installer(
            self.dest,
            make_controlled_path(self.base),
            home=self.home,
            shell="/no/such/shell",
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

    def test_an_unset_shell_reads_the_login_shell_interactive_profile(self) -> None:
        # SHELL may be absent. The old fallback was /bin/sh, and on most Linux
        # hosts that is dash, which reads neither ~/.bashrc nor ~/.zshrc. A
        # machine whose install wrote the PATH line there therefore read as
        # inactive. The installer must resolve the account's login shell instead
        # and ask the profile that shell reads interactively. Only the
        # interactive profile is written here: ~/.profile would let the old
        # fallback pass and hide the regression.
        shell = account_login_shell()
        if shell is None:
            self.skipTest("could not determine the account login shell")
        name = Path(shell).name
        if name not in ("bash", "zsh"):
            self.skipTest(f"login shell {shell!r} is neither bash nor zsh")
        if shutil.which(name) is None:
            self.skipTest(f"login shell {shell!r} is not installed")
        profile = ".bashrc" if name == "bash" else ".zshrc"
        (self.home / profile).write_text(
            f'export PATH="{self.dest}:$PATH"\n', encoding="utf-8"
        )
        result = run_installer(
            self.dest, make_controlled_path(self.base), home=self.home, drop_shell=True
        )
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertIn("shim is active", output)

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
