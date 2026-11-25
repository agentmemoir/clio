#!/usr/bin/env python3
"""Tests for the release-manifest signature check in scripts/install.sh.

Why this exists
---------------
The installer downloads `checksums.txt` and the asset from one release, so a
hash in that file proves only that the two downloads agree: whoever can replace
one can replace both. The installer verifies a detached RSA signature over
`checksums.txt` against a public key built into itself, and stops before it
downloads the asset or installs anything when the signature does not verify.

Two groups of cases. The first calls the verifier on its own, through the
`CLIO_INSTALL_LIB_ONLY=1` seam, to pin each message. The second runs the whole
installer against a fixture release: a stub `curl` and `wget` serve files by
basename from a temp release directory and a stub `uname` fixes the target, so
`main` runs for real, with no network, and a deleted `verify_manifest` call in
`main` fails these cases. That same harness pins the install step the signature
guards: a staging name another account planted is not written through, and a
directory other users can write to is refused before anything is downloaded.

Each case makes a throwaway keypair and copies the installer with its embedded
key replaced by the throwaway public key, so nothing here depends on the host's
key material. What the real file carries is asked separately: the shipped key
must parse as a public key.

Run:
    python3 scripts/test_install_manifest.py
"""

from __future__ import annotations

import hashlib
import os
import shutil
import subprocess
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from exec_tempdir import exec_tempdir  # noqa: E402

INSTALLER = Path(__file__).resolve().parent / "install.sh"
SH = shutil.which("sh") or "/bin/sh"
OPENSSL = shutil.which("openssl")

BEGIN = "-----BEGIN PUBLIC KEY-----"
END = "-----END PUBLIC KEY-----"
TARGET = "x86_64-unknown-linux-musl"

FIXTURE = (
    "3f786850e387550fdab836ed7e6dc881de23001b  clio-x86_64-unknown-linux-musl.tar.gz\n"
    "89e6c98d92887913cadf06b2adb97f26cde4849b  clio-aarch64-unknown-linux-musl.tar.gz\n"
)

#: A stub downloader. It maps the last URL argument to a file of the same
#: basename in the fixture release directory, so the installer's own choice of
#: curl or wget decides which stub runs and nothing reaches the network.
DOWNLOADER = """#!/bin/sh
out=""
url=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o|-O) out="$2"; shift 2 ;;
    -*) shift ;;
    *) url="$1"; shift ;;
  esac
done
[ -n "$out" ] || exit 2
[ -n "$url" ] || exit 2
src="{release}/${{url##*/}}"
[ -f "$src" ] || exit 22
cp "$src" "$out"
"""

UNAME = """#!/bin/sh
case "$1" in
  -m|-p|--machine) echo x86_64 ;;
  *) echo Linux ;;
esac
"""


def openssl(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [OPENSSL, *args], capture_output=True, text=True, check=False
    )


def sha256_of(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def make_keypair(base: Path) -> tuple[Path, Path]:
    """A throwaway RSA keypair, written in the temp tree."""
    key = base / "signing-key.pem"
    pub = base / "signing-key.pub.pem"
    done = openssl(
        "genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048", "-out", str(key)
    )
    if done.returncode != 0:
        raise RuntimeError(f"could not generate a test key: {done.stderr}")
    done = openssl("pkey", "-in", str(key), "-pubout", "-out", str(pub))
    if done.returncode != 0:
        raise RuntimeError(f"could not read the test public key: {done.stderr}")
    return key, pub


def sign(key: Path, data: Path, out: Path) -> None:
    done = openssl("dgst", "-sha256", "-sign", str(key), "-out", str(out), str(data))
    if done.returncode != 0:
        raise RuntimeError(f"could not sign the fixture: {done.stderr}")


def embedded_key(text: str) -> str:
    """The public key PEM an installer text carries, verbatim."""
    start = text.find(BEGIN)
    end = text.find(END, start)
    if start < 0 or end < 0:
        raise AssertionError("the installer carries no public key block")
    return text[start : end + len(END)]


def copy_installer(dest: Path, public_key: Path) -> Path:
    """A copy of the installer whose embedded key is the throwaway one."""
    text = INSTALLER.read_text(encoding="utf-8")
    old = embedded_key(text)
    new = public_key.read_text(encoding="utf-8").strip()
    replaced = text.replace(old, new)
    if replaced == text:
        raise AssertionError("the embedded key was not replaced")
    dest.write_text(replaced, encoding="utf-8")
    return dest


def stub(path: Path, body: str) -> None:
    path.write_text(body, encoding="utf-8")
    path.chmod(0o755)


def build_release(base: Path, key: Path) -> Path:
    """A fixture release: archive, manifest, and a signature over the manifest."""
    release = base / "release"
    release.mkdir()
    root = base / f"clio-{TARGET}"
    root.mkdir()
    binary = root / "clio"
    binary.write_text("#!/bin/sh\necho fixture-clio\n", encoding="utf-8")
    binary.chmod(0o755)
    archive = release / f"clio-{TARGET}.tar.gz"
    subprocess.run(
        ["tar", "-czf", str(archive), "-C", str(base), f"clio-{TARGET}"], check=True
    )
    manifest = release / "checksums.txt"
    manifest.write_text(f"{sha256_of(archive)}  {archive.name}\n", encoding="utf-8")
    sign(key, manifest, release / "checksums.txt.sig")
    return release


@unittest.skipIf(
    OPENSSL is None and not os.environ.get("CI"),
    "openssl is not installed; CI never skips this suite",
)
class ManifestSignatureTests(unittest.TestCase):
    def setUp(self) -> None:
        # The stub tools must run, so the tree goes in a directory the host
        # really executes from: CI mounts its temporary storage noexec, and a
        # stub there is invisible to `command -v` and never runs.
        tmp = exec_tempdir(purpose="the release manifest installer test")
        self.addCleanup(tmp.cleanup)
        self.base = Path(tmp.name)
        self.workdir = self.base / "work"
        self.workdir.mkdir()
        self.install_dir = self.base / "install"
        self.key, self.pub = make_keypair(self.base)
        self.installer = copy_installer(self.base / "installer.sh", self.pub)
        self.manifest = self.base / "checksums.txt"
        self.manifest.write_text(FIXTURE, encoding="utf-8")
        self.signature = self.base / "checksums.txt.sig"
        sign(self.key, self.manifest, self.signature)

    # --- the verifier on its own ---------------------------------------------

    def verify(self, signature: Path) -> subprocess.CompletedProcess[str]:
        """Source the installer and call the verifier on the fixture manifest."""
        script = '. "$1"\nworkdir="$2"\nverify_manifest "$3" "$4"\n'
        env = dict(os.environ)
        env["CLIO_INSTALL_LIB_ONLY"] = "1"
        env["CLIO_INSTALL_DIR"] = str(self.install_dir)
        return subprocess.run(
            [
                SH, "-c", script, "sh",
                str(self.installer), str(self.workdir),
                str(self.manifest), str(signature),
            ],
            capture_output=True,
            text=True,
            env=env,
            check=False,
        )

    def test_a_valid_signature_passes(self) -> None:
        done = self.verify(self.signature)
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertNotIn("error:", done.stderr)

    def test_a_signature_over_other_bytes_fails(self) -> None:
        # The same key signs a different manifest, so the signature is well
        # formed and still must not vouch for this one.
        other = self.base / "other-checksums.txt"
        other.write_text(FIXTURE.replace("3f7868", "000000"), encoding="utf-8")
        signature = self.base / "other-checksums.txt.sig"
        sign(self.key, other, signature)
        done = self.verify(signature)
        self.assertNotEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn("does not match the key", done.stderr)

    def test_a_missing_signature_file_fails(self) -> None:
        done = self.verify(self.base / "checksums.txt.sig.missing")
        self.assertNotEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn("missing or is not a regular file", done.stderr)

    def test_a_directory_in_place_of_the_signature_fails(self) -> None:
        directory = self.base / "signature-is-a-directory.sig"
        directory.mkdir()
        done = self.verify(directory)
        self.assertNotEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn("not a regular file", done.stderr)

    def test_an_empty_signature_file_fails(self) -> None:
        empty = self.base / "empty.sig"
        empty.write_bytes(b"")
        done = self.verify(empty)
        self.assertNotEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn("is empty", done.stderr)

    def test_the_shipped_key_parses(self) -> None:
        key = embedded_key(INSTALLER.read_text(encoding="utf-8"))
        path = self.base / "shipped.pub.pem"
        path.write_text(key, encoding="utf-8")
        done = openssl("pkey", "-pubin", "-in", str(path), "-noout")
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)

    # --- the whole installer -------------------------------------------------

    def install(self, release: Path) -> subprocess.CompletedProcess[str]:
        """Run main against the fixture release, with no network and no host target."""
        bindir = self.base / "bin"
        bindir.mkdir(exist_ok=True)
        body = DOWNLOADER.format(release=release)
        for name in ("curl", "wget"):
            stub(bindir / name, body)
        stub(bindir / "uname", UNAME)
        home = self.base / "home"
        home.mkdir(exist_ok=True)
        env = dict(os.environ)
        env["PATH"] = os.pathsep.join((str(bindir), env.get("PATH", "/usr/bin:/bin")))
        env["HOME"] = str(home)
        env["CLIO_INSTALL_DIR"] = str(self.install_dir)
        env.pop("CLIO_INSTALL_LIB_ONLY", None)
        return subprocess.run(
            [SH, str(self.installer)],
            capture_output=True,
            text=True,
            env=env,
            check=False,
        )

    def assert_nothing_installed(self, done: subprocess.CompletedProcess[str]) -> None:
        installed = self.install_dir / "clio"
        self.assertFalse(
            installed.exists(),
            "a failed verification installed a binary\n" + done.stdout + done.stderr,
        )

    def test_main_installs_the_binary_from_a_signed_release(self) -> None:
        done = self.install(build_release(self.base, self.key))
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        installed = self.install_dir / "clio"
        self.assertTrue(installed.exists(), done.stdout + done.stderr)
        self.assertTrue(os.access(installed, os.X_OK))
        self.assertIn("checksum ok", done.stderr)
        self.assertIn("installed", done.stderr)

    def test_main_refuses_a_bad_signature_before_it_downloads_the_asset(self) -> None:
        release = build_release(self.base, self.key)
        other = self.base / "other.txt"
        other.write_text(FIXTURE, encoding="utf-8")
        sign(self.key, other, release / "checksums.txt.sig")
        done = self.install(release)
        self.assertNotEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn("does not match the key", done.stderr)
        self.assertNotIn("downloading clio-", done.stderr)
        self.assert_nothing_installed(done)

    def test_library_mode_says_so_and_installs_nothing(self) -> None:
        release = build_release(self.base, self.key)
        bindir = self.base / "bin"
        bindir.mkdir(exist_ok=True)
        env = dict(os.environ)
        env["PATH"] = os.pathsep.join((str(bindir), env.get("PATH", "/usr/bin:/bin")))
        env["CLIO_INSTALL_LIB_ONLY"] = "1"
        env["CLIO_INSTALL_DIR"] = str(self.install_dir)
        done = subprocess.run(
            [SH, str(self.installer)], capture_output=True, text=True, env=env, check=False
        )
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn("library mode, so nothing was installed", done.stderr)
        self.assert_nothing_installed(done)

    # --- the install directory and the staging file --------------------------

    def test_a_planted_staging_entry_is_not_followed(self) -> None:
        release = build_release(self.base, self.key)
        self.install_dir.mkdir(parents=True)
        self.install_dir.chmod(0o755)
        # The fixed name the installer used to write, and a name of the shape it
        # now asks mktemp for.
        planted = []
        for index, name in enumerate((".clio.new", ".clio.new.AAAAAA")):
            victim = self.base / f"victim-{index}"
            victim.write_text("original profile\n", encoding="utf-8")
            victim.chmod(0o600)
            (self.install_dir / name).symlink_to(victim)
            planted.append((self.install_dir / name, victim))

        done = self.install(release)

        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        for entry, victim in planted:
            self.assertEqual(victim.read_text(encoding="utf-8"), "original profile\n")
            self.assertEqual(victim.stat().st_mode & 0o777, 0o600)
            self.assertTrue(entry.is_symlink(), f"{entry.name} was replaced")
        installed = self.install_dir / "clio"
        self.assertFalse(
            installed.is_symlink(), "the installed entry is a link to something else"
        )
        self.assertTrue(os.access(installed, os.X_OK), done.stdout + done.stderr)

    def test_a_shared_install_dir_is_refused_before_downloading(self) -> None:
        release = build_release(self.base, self.key)
        self.install_dir.mkdir(parents=True)
        self.install_dir.chmod(0o777)

        done = self.install(release)

        self.assertNotEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn("is writable by group or other", done.stderr)
        self.assertNotIn("downloading", done.stderr)
        self.assert_nothing_installed(done)
        self.assertEqual(list(self.install_dir.iterdir()), [])

    def check_install_dir(
        self, path: Path, prelude: str = ""
    ) -> subprocess.CompletedProcess[str]:
        """Source the installer and run its install-directory check on one path."""
        script = f'. "$1"\nINSTALL_DIR="$2"\n{prelude}check_install_dir\n'
        env = dict(os.environ)
        env["CLIO_INSTALL_LIB_ONLY"] = "1"
        return subprocess.run(
            [SH, "-c", script, "sh", str(self.installer), str(path)],
            capture_output=True,
            text=True,
            env=env,
            check=False,
        )

    def test_a_mode_other_users_can_write_to_is_refused(self) -> None:
        cases = (
            (0o700, False),
            (0o750, False),
            (0o755, False),
            (0o775, True),
            (0o777, True),
        )
        for mode, refused in cases:
            shared = self.base / f"mode-{mode:o}"
            shared.mkdir()
            shared.chmod(mode)
            done = self.check_install_dir(shared)
            self.assertEqual(
                done.returncode != 0,
                refused,
                f"mode {mode:o}: " + done.stdout + done.stderr,
            )

    def test_stat_output_is_read_as_octal_or_refused(self) -> None:
        # macOS stat has no -c and spells the mode %Lp, so the digits arrive as
        # octal text either way, and output that is not a mode and an owner is
        # an error rather than a directory that passes.
        for mode, refused in (("700", False), ("775", True), ("not-a-mode", True)):
            prelude = (
                f"mode={mode}\n"
                'stat() { case "$1" in -c) return 1 ;;'
                ' -f) printf "%s %s\\n" "$mode" "$(id -u)" ;; esac; }\n'
            )
            done = self.check_install_dir(self.base / "directory", prelude)
            self.assertEqual(
                done.returncode != 0,
                refused,
                f"stat output {mode!r}: " + done.stdout + done.stderr,
            )

    def test_a_directory_owned_by_another_account_warns(self) -> None:
        done = self.check_install_dir(self.base, 'id() { printf "0\\n"; }\n')
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn("is not owned by you", done.stderr)


if __name__ == "__main__":
    unittest.main()
