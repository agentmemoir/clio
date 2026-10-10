"""Pinned snapshot fetch and verification.

Datasets are downloaded at benchmark time and never committed. Every download
is verified against the pin recorded in `pins.json`; a mismatch raises
`PinError` and the runner aborts before producing any score.
"""

from __future__ import annotations

import hashlib
import json
import urllib.request
from pathlib import Path

_CHUNK = 1 << 20


class PinError(RuntimeError):
    """Raised when a downloaded snapshot does not match its recorded pin."""


class MissingSnapshotError(RuntimeError):
    """Raised when a required pinned snapshot cannot be resolved."""


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(_CHUNK), b""):
            digest.update(chunk)
    return digest.hexdigest()


def git_blob_sha1(data: bytes) -> str:
    """Git blob object id: sha1 over `blob <len>\\0<content>`."""
    header = f"blob {len(data)}\0".encode("ascii")
    return hashlib.sha1(header + data).hexdigest()  # noqa: S324 - git object id


def load_pins(pins_path: Path) -> dict:
    with open(pins_path, "r", encoding="utf-8") as handle:
        return json.load(handle)


def _download(url: str, dest: Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(dest.suffix + ".part")
    with urllib.request.urlopen(url, timeout=120) as response, open(tmp, "wb") as out:
        while True:
            chunk = response.read(_CHUNK)
            if not chunk:
                break
            out.write(chunk)
    tmp.replace(dest)


def verify_file(path: Path, pin: dict) -> None:
    """Verify `path` against the `sha256` or `git_blob` fields of `pin`."""
    expected_sha = pin.get("sha256")
    expected_blob = pin.get("git_blob")
    if expected_sha:
        actual = sha256_file(path)
        if actual != expected_sha:
            raise PinError(
                f"{path.name}: sha256 {actual} != pinned {expected_sha}"
            )
    if expected_blob:
        with open(path, "rb") as handle:
            actual_blob = git_blob_sha1(handle.read())
        if actual_blob != expected_blob:
            raise PinError(
                f"{path.name}: git blob {actual_blob} != pinned {expected_blob}"
            )


def ensure_dataset(pin: dict, cache_dir: Path) -> Path:
    """Return a local path to the pinned file, downloading and verifying once."""
    filename = pin["filename"]
    path = cache_dir / filename
    if path.exists():
        verify_file(path, pin)
        return path
    try:
        _download(pin["url"], path)
    except Exception as exc:  # noqa: BLE001 - surface any transport failure clearly
        raise MissingSnapshotError(
            f"could not download pinned snapshot {filename} from {pin['url']}: {exc}"
        ) from exc
    verify_file(path, pin)
    return path