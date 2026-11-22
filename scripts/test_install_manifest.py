#!/usr/bin/env python3
"""Tests for the release-manifest signature check in scripts/install.sh.

Why this exists
---------------
The installer downloads `checksums.txt` and the asset from one release, so a
hash in that file proves only that the two downloads agree: whoever can replace
one can replace both. The installer now verifies a detached RSA signature over
`checksums.txt` against a public key built into itself, and stops before it
downloads the asset or installs anything when the signature does not verify.

These cases call the verifier directly, through the `CLIO_INSTALL_LIB_ONLY=1`
seam, so they need no release and no network. Each case makes a throwaway
keypair, signs a fixture manifest with it, and copies the installer with its
embedded key replaced by the throwaway public key: the verified key is a temp
path and nothing here depends on the host. What the real file carries is asked
separately: the shipped key must parse as a public key.

Run:
    python3 scripts/test_install_manifest.py
"""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

INSTALLER = Path(__file__).resolve().parent / "install.sh"
SH = shutil.which("sh") or "/bin/sh"
OPENSSL = shutil.which("openssl")

BEGIN = "-----BEGIN PUBLIC KEY-----"
END = "-----END PUBLIC KEY-----"

FIXTURE = (
    "3f786850e387550fdab836ed7e6dc881de23001b  clio-x86_64-unknown-linux-musl.tar.gz\n"
    "89e6c98d92887913cadf06b2adb97f26cde4849b  clio-aarch64-unknown-linux-musl.tar.gz\n"
)


def openssl(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [OPENSSL, *args], capture_output=True, text=True, check=False
    )


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


@unittest.skipIf(OPENSSL is None, "openssl is not installed")
class ManifestSignatureTests(unittest.TestCase):
    def setUp(self) -> None:
        self.base = Path(tempfile.mkdtemp(prefix="clio-install-manifest-"))
        self.addCleanup(shutil.rmtree, self.base, True)
        self.workdir = self.base / "work"
        self.workdir.mkdir()
        self.install_dir = self.base / "install"
        self.key, self.pub = make_keypair(self.base)
        self.installer = copy_installer(self.base / "installer.sh", self.pub)
        self.manifest = self.base / "checksums.txt"
        self.manifest.write_text(FIXTURE, encoding="utf-8")
        self.signature = self.base / "checksums.txt.sig"
        sign(self.key, self.manifest, self.signature)

    def verify(self, signature: Path) -> subprocess.CompletedProcess[str]:
        """Source the installer and call the verifier on the fixture manifest."""
        script = '. "$1"\nworkdir="$2"\nverify_manifest "$3" "$4"\n'
        env = dict(os.environ)
        env["CLIO_INSTALL_LIB_ONLY"] = "1"
        env["CLIO_INSTALL_DIR"] = str(self.install_dir)
        return subprocess.run(
            [
                SH,
                "-c",
                script,
                "sh",
                str(self.installer),
                str(self.workdir),
                str(self.manifest),
                str(signature),
            ],
            capture_output=True,
            text=True,
            env=env,
            check=False,
        )

    def assert_nothing_installed(self, done: subprocess.CompletedProcess[str]) -> None:
        self.assertFalse(
            (self.install_dir / "clio").exists(),
            "a failed verification installed a binary\n" + done.stdout + done.stderr,
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
        self.assert_nothing_installed(done)

    def test_a_missing_signature_file_fails(self) -> None:
        done = self.verify(self.base / "checksums.txt.sig.missing")
        self.assertNotEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn("missing or empty", done.stderr)
        self.assert_nothing_installed(done)

    def test_an_empty_signature_file_fails(self) -> None:
        empty = self.base / "empty.sig"
        empty.write_bytes(b"")
        done = self.verify(empty)
        self.assertNotEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertIn("missing or empty", done.stderr)
        self.assert_nothing_installed(done)

    def test_the_shipped_key_parses(self) -> None:
        key = embedded_key(INSTALLER.read_text(encoding="utf-8"))
        path = self.base / "shipped.pub.pem"
        path.write_text(key, encoding="utf-8")
        done = openssl("pkey", "-pubin", "-in", str(path), "-noout")
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)


if __name__ == "__main__":
    unittest.main()
