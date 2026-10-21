#!/usr/bin/env python3
"""Tests for the profile probe of scripts/gate-install.sh.

Split out of test_gate_install.py to keep each file under the 450-line limit. The cases here ask a shell which profile it reads, ask which shell the installer falls back to when SHELL is unusable, and pin the timeout tool that bounds the probe: the 124 case, the gtimeout case, the five second budget, and the order the two tool names are tried in.

The last set of cases asks the runner check script, the one CI runs on a Mac, to accept the two outcomes a host can have, so the script that guards the installer is itself guarded.

    python3 scripts/test_gate_install_profiles.py
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from gate_install_support import (  # noqa: E402
    BASH,
    SH,
    SYSTEM_PATH,
    ZSH,
    account_login_shell,
    install_tree,
    make_controlled_path,
    make_recording_timeout_bin,
    make_timeout_bin,
    run_installer,
    stub_tool,
    write_profile_path_line,
)


class GateInstallProfileTests(unittest.TestCase):
    def setUp(self) -> None:
        self.base, self.dest, self.earlier, self.home = install_tree(self)

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
        write_profile_path_line(self.home, self.dest)
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

    def test_the_probe_passes_the_five_second_budget_to_the_timeout_tool(self) -> None:
        # The installer bounds a profile probe with a 5 second budget, so the duration it hands the timeout tool is part of the contract. The stand-in records the arguments it was given, so this pins the budget and not only the name of the tool.
        if BASH is None:
            self.skipTest("no bash to act as the login shell")
        record = self.base / "timeout-args"
        toolbin = make_recording_timeout_bin(self.base, "timeout", record)
        write_profile_path_line(self.home, self.dest)
        result = run_installer(
            self.dest,
            os.pathsep.join((str(toolbin), SYSTEM_PATH)),
            home=self.home,
            shell=BASH,
        )
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertTrue(record.exists(), "the timeout tool never ran")
        args = record.read_text(encoding="utf-8").splitlines()
        self.assertEqual(args[0], "5", f"the probe budget was {args[0]!r}, not 5")
        self.assertIn("command -v", " ".join(args))

    def test_the_plain_timeout_wins_when_both_tools_are_on_path(self) -> None:
        # The installer looks for `timeout` first and then `gtimeout`, so on a host that has both the plain name wins. The two stand-ins behave differently: the `timeout` one runs the command it is given, the `gtimeout` one does not. The test passes only when the plain `timeout` was the tool that ran.
        if BASH is None:
            self.skipTest("no bash to act as the login shell")
        plain_record = self.base / "plain-args"
        other_record = self.base / "other-args"
        plain = make_recording_timeout_bin(self.base, "timeout", plain_record)
        other = make_recording_timeout_bin(
            self.base, "gtimeout", other_record, exec_command=False
        )
        write_profile_path_line(self.home, self.dest)
        result = run_installer(
            self.dest,
            os.pathsep.join((str(plain), str(other), SYSTEM_PATH)),
            home=self.home,
            shell=BASH,
        )
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertTrue(plain_record.exists(), "the plain timeout tool never ran")
        self.assertFalse(other_record.exists(), "gtimeout ran, so the order is wrong")

    def test_a_probe_without_a_timeout_or_gtimeout_tool_is_not_run(self) -> None:
        # There is no portable profile watchdog: GNU coreutils installs it as `timeout` on Linux and as `gtimeout` on macOS. If neither tool is available, the installer must leave a blocking profile unprobed.
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


class GateInstallRunnerCheckTests(unittest.TestCase):
    """Tests for scripts/gate_install_runner_check.sh.

    The script runs the real installer the way a user runs it and accepts only the two outcomes a host can have: a host with a timeout tool must see the installer confirm the shim, and a host without one must see the installer report that profile verification is unavailable. These cases drive it with a controlled PATH, so the branch the script takes is the branch the test asks for and not the branch this host would give.
    """

    def setUp(self) -> None:
        self.base, self.dest, self.earlier, self.home = install_tree(self)
        self.runner = Path(__file__).resolve().parent / "gate_install_runner_check.sh"
        # The tools the script and the installer call. The PATH holds these and
        # nothing else, so the answer does not depend on what the host has
        # installed, and a host that has a real timeout tool cannot put the
        # script into a branch the test did not ask for.
        self.toolbin = self.base / "toolbin"
        self.toolbin.mkdir()
        for name in ("chmod", "dirname", "ln", "mkdir", "mktemp", "readlink", "rm", "sh"):
            source = shutil.which(name, path=SYSTEM_PATH)
            self.assertIsNotNone(source, f"the host has no {name} to give the script")
            os.symlink(source, self.toolbin / name)
        # The directory the script builds its own temporary home in. The script
        # removes that home on every exit, and the cases check that it did.
        self.tmpdir = self.base / "tmp"
        self.tmpdir.mkdir()

    def run_check(self, path: str) -> subprocess.CompletedProcess[str]:
        """The runner check script, run with the given PATH and a temporary directory of its own.

        The run is bounded twice. The script wraps the installer in its own timeout tool and gives it 120 seconds, and this call gives the script 150 seconds, so a run that hangs fails the case instead of holding the suite.
        """
        env = dict(os.environ)
        env["PATH"] = path
        env["TMPDIR"] = str(self.tmpdir)
        try:
            result = subprocess.run(
                [SH, str(self.runner)],
                capture_output=True,
                text=True,
                env=env,
                check=False,
                timeout=150,
            )
        except subprocess.TimeoutExpired:
            self.fail("the runner check script did not finish inside 150 seconds")
        leftovers = sorted(path.name for path in self.tmpdir.iterdir())
        self.assertEqual(leftovers, [], "the script left a temporary directory behind")
        return result

    def test_a_host_with_a_timeout_tool_sees_the_installer_confirm_the_shim(self) -> None:
        # A host with a watchdog lets the installer confirm the shim, so the script must accept that outcome and must say that it found the tool. The stand-in timeout tool stands in for the one macOS does not ship.
        standin = make_timeout_bin(self.base, "timeout")
        result = self.run_check(os.pathsep.join((str(standin), str(self.toolbin))))
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertIn("the timeout tool on this host: timeout", output)
        self.assertIn("shim is active", output)
        self.assertIn("the installer behaved as this host requires", output)

    def test_a_host_without_a_timeout_tool_sees_the_installer_refuse_to_probe(self) -> None:
        # A host with no watchdog cannot probe a profile, so the installer must fail and say that profile verification is unavailable. That is a correct outcome, so the script accepts it and the script itself exits zero.
        result = self.run_check(str(self.toolbin))
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertIn("the timeout tool on this host: none", output)
        self.assertIn("profile verification is unavailable", output)
        self.assertIn("the installer behaved as this host requires", output)

    def test_an_outcome_the_host_does_not_allow_makes_the_script_fail(self) -> None:
        # The script is only worth running when it rejects an outcome the host does not allow. A timeout tool that always reports a killed probe leaves the shim unconfirmed, so the installer does not say the shim is active and the script must fail.
        fakebin = self.base / "fakebin"
        fakebin.mkdir()
        fake = fakebin / "timeout"
        fake.write_text(
            '#!/bin/sh\nif [ "$1" = 5 ]; then exit 124; fi\nshift\nexec "$@"\n',
            encoding="utf-8",
        )
        fake.chmod(0o755)
        result = self.run_check(os.pathsep.join((str(fakebin), str(self.toolbin))))
        output = result.stdout + result.stderr
        self.assertNotEqual(result.returncode, 0, output)
        self.assertIn("the installer must report that the shim is active", output)


if __name__ == "__main__":
    unittest.main()
