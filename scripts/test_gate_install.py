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
exits zero when both names resolve to the shim. It also asks the login and
interactive profile shells, keeps a killed (slow) probe from reading as
inactive, and names a login/interactive divergence instead of hiding it. Most
tests build a controlled PATH and set SHELL, so their answer does not depend on
what the host has installed. The login-shell tests instead ask the host's
account login shell and skip when it cannot be determined.
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
ZSH = shutil.which("zsh")

#: The directories that hold the external commands the installer itself calls
#: (`dirname`, `mkdir`, `ln`, `chmod`, `readlink`). They are kept on PATH so the
#: controlled PATH does not also hide the tools the script needs to run.
SYSTEM_PATH = os.pathsep.join(("/usr/bin", "/bin"))


def account_login_shell() -> str | None:
    """The account's login shell, as the installer resolves it when SHELL is unset.

    The installer falls back to the account's login shell, so a test that
    exercises that path must resolve the same shell to know which profile it
    reads. The lookup runs with the installer's PATH, so the helper and the
    installer see the same `id`, `getent`, and `dscl`. `getent` is the Linux
    query and `dscl` the macOS one. Returns None when neither can answer.
    """
    env = dict(os.environ)
    env["PATH"] = SYSTEM_PATH
    done = subprocess.run(
        ["id", "-un"], capture_output=True, text=True, check=False, env=env
    )
    account = done.stdout.strip()
    if done.returncode != 0 or not account:
        return None
    getent = shutil.which("getent", path=SYSTEM_PATH)
    if getent is not None:
        done = subprocess.run(
            [getent, "passwd", account],
            capture_output=True,
            text=True,
            check=False,
            env=env,
        )
        fields = done.stdout.strip().split(":")
        if done.returncode == 0 and len(fields) >= 7 and fields[6]:
            return fields[6]
        return None
    dscl = shutil.which("dscl", path=SYSTEM_PATH)
    if dscl is not None:
        done = subprocess.run(
            [dscl, ".", "-read", f"/Users/{account}", "UserShell"],
            capture_output=True,
            text=True,
            check=False,
            env=env,
        )
        if done.returncode == 0:
            for line in done.stdout.splitlines():
                if line.startswith("UserShell:"):
                    return line.split(":", 1)[1].strip()
    return None


def run_installer(
    dest: Path,
    path: str,
    home: Path | None = None,
    shell: str | None = SH,
    drop_shell: bool = False,
) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env["PATH"] = path
    if home is not None:
        env["HOME"] = str(home)
    if drop_shell:
        env.pop("SHELL", None)
    else:
        # Pin SHELL so the probed shell is the same on every host. Leaving it
        # unset lets /bin/sh fill it from the passwd database on some hosts.
        env["SHELL"] = shell or SH
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

    def test_a_profile_that_prints_does_not_hide_the_shim(self) -> None:
        # A profile often prints to stdout. The probe must find the shim among
        # the printed lines, or a working machine reads as inactive.
        if BASH is None:
            self.skipTest("no bash to act as the interactive shell")
        (self.home / ".bashrc").write_text(
            f'echo "welcome back"\nexport PATH="{self.dest}:$PATH"\n',
            encoding="utf-8",
        )
        result = run_installer(self.dest, SYSTEM_PATH, home=self.home, shell=BASH)
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
        result = run_installer(self.dest, SYSTEM_PATH, home=self.home, shell=ZSH)
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
        result = run_installer(self.dest, SYSTEM_PATH, home=self.home, shell=ZSH)
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertIn("shim is active", output)

    def test_a_probe_timeout_is_not_read_as_inactive(self) -> None:
        # A profile slower than the 5s probe budget is killed by `timeout`,
        # which exits 124. That means "could not probe", not "the shim is
        # inactive", so it must not fail an otherwise correct install (#213).
        # A stand-in `timeout` that returns 124 pins this without a real sleep.
        fakebin = Path(self._tmp.name) / "fakebin"
        fakebin.mkdir()
        fake = fakebin / "timeout"
        fake.write_text("#!/bin/sh\nexit 124\n", encoding="utf-8")
        fake.chmod(0o755)
        self.stub_tool("cargo")
        self.stub_tool("make")
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

    def test_a_login_hit_with_an_interactive_shadow_names_the_divergence(self) -> None:
        # A login profile can add `dest` while the interactive profile puts a
        # real tool first. The login shell counts, so the install stays active,
        # but the shell the user runs gates in does not see the shim; that must
        # be named rather than hidden behind a bare "active in a login shell".
        if BASH is None:
            self.skipTest("no bash to act as the login and interactive shell")
        self.stub_tool("cargo")
        self.stub_tool("make")
        (self.home / ".profile").write_text(
            f'export PATH="{self.dest}:$PATH"\n', encoding="utf-8"
        )
        (self.home / ".bashrc").write_text(
            f'export PATH="{self.earlier}:$PATH"\n', encoding="utf-8"
        )
        result = run_installer(self.dest, SYSTEM_PATH, home=self.home, shell=BASH)
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertIn("not active in the interactive shell", output)

    def test_an_unset_shell_does_not_break_the_install(self) -> None:
        # SHELL may be absent. The installer must still run. On macOS the
        # launching `sh` refills SHELL from the passwd database, so which shell
        # is chosen is not observable here; the install must simply succeed.
        result = run_installer(
            self.dest,
            os.pathsep.join((str(self.dest), SYSTEM_PATH, str(self.earlier))),
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
            self.dest, SYSTEM_PATH, home=self.home, shell="/no/such/shell"
        )
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode, 0, output)
        self.assertIn("shim is active", output)

    def test_an_unresolvable_login_shell_falls_back_to_bin_sh(self) -> None:
        # Force the account lookup to fail: `id`, `getent`, and `dscl` are
        # stubs that exit 1, and SHELL is unusable. The last resort is /bin/sh;
        # no profile on its PATH adds `dest`, so the install must report
        # NOT ACTIVE and exit non-zero instead of reading the shim as active.
        fakebin = Path(self._tmp.name) / "noid"
        fakebin.mkdir()
        for tool in ("id", "getent", "dscl"):
            stub = fakebin / tool
            stub.write_text("#!/bin/sh\nexit 1\n", encoding="utf-8")
            stub.chmod(0o755)
        result = run_installer(
            self.dest,
            os.pathsep.join((str(fakebin), SYSTEM_PATH)),
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
        result = run_installer(self.dest, SYSTEM_PATH, home=self.home, drop_shell=True)
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
