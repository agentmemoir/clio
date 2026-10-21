#!/usr/bin/env python3
"""The shared helpers of the gate-installer self-test.

Why these exist
---------------
The installer links `cargo` and `make` to the gate shim in a directory the
caller later puts on PATH. A symlink in a directory that comes *after* the real
tool never runs, so the shim can be installed and dead. The installer used to
test only that its directory appeared somewhere on PATH and printed a success
line either way, so a dead shim read as a clean install and every later run
quoted a verdict no live run had refreshed.

The self-test in `scripts/test_gate_install.py` pins the correction: the
installer resolves each name, reports NOT ACTIVE and exits non-zero when a name
lands outside its directory, and exits zero when both names resolve to the shim.
It also asks the login and interactive profile shells, keeps a killed (slow)
probe from reading as inactive, and names a login/interactive divergence instead
of hiding it. Most tests build a controlled PATH and set SHELL, so their answer
does not depend on what the host has installed. The login-shell tests instead
ask the host's account login shell and skip when it cannot be determined.

The helpers here are the part every case needs: the installer path, the shells
the installer resolves, the account login shell it falls back to, the
controlled PATH a case runs against, and the stand-in timeout tool macOS lacks.
They live in their own module so the test file holds the cases and nothing else,
and so a file that outgrows the 450-line limit is split along a line a reader
can name.

Run the self-test:

    python3 scripts/test_gate_install.py
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

#: Re-exported so the self-test imports every shared name from one place.
from exec_tempdir import exec_tempdir  # noqa: E402,F401

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
    timeout: float | None = None,
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
        timeout=timeout,
    )


def stub_tool(directory: Path, name: str) -> None:
    """A stand-in for a real tool in the directory that comes first on PATH."""
    path = directory / name
    path.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    path.chmod(0o755)


def make_timeout_bin(base: Path, name: str = "timeout") -> Path:
    """The directory that holds a stand-in timeout tool named `name`.

    The installer bounds a profile probe with a timeout tool. macOS has no
    `timeout`, and GNU coreutils installs the same tool as `gtimeout`, so
    the test builds its own stand-in instead of relying on the host. The
    stand-in drops the duration argument and runs the rest, so a probe
    behaves as if the command finished inside the budget.
    """
    toolbin = base / f"{name}bin"
    toolbin.mkdir(exist_ok=True)
    tool = toolbin / name
    tool.write_text('#!/bin/sh\nshift\nexec "$@"\n', encoding="utf-8")
    tool.chmod(0o755)
    return toolbin


def make_controlled_path(base: Path) -> str:
    """A PATH with the timeout stand-in and the system tools, and nothing else.

    The host may have no timeout tool at all, so the stand-in goes on the
    controlled PATH. The answer of a test then depends on the tree under
    test, not on what the host has installed.
    """
    return os.pathsep.join((str(make_timeout_bin(base)), SYSTEM_PATH))
