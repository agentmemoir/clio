"""Pinned, offline cl100k_base tokenizer for budget measurement.

The encoding is loaded only from a repo-committed, checksum-verified asset, so a
budget number can never come from a regex heuristic or a network fetch.
"""

from __future__ import annotations

import hashlib
import os
import shutil
import tempfile
from pathlib import Path

ENCODING = "cl100k_base"
ASSET_NAME = "cl100k_base.tiktoken"
ASSET_SHA256 = "223921b76ee99bde995b7ff738513eef100fb51d18c93597a113bcffe865b2a7"
_CACHE_KEY = "9b5ad71b2ce5302211f9c61530b329a4922fc6a4"

_ASSET_PATH = Path(__file__).resolve().parent / "assets" / ASSET_NAME

#: Message the network guard raises when tiktoken tries to read a remote blob.
NETWORK_GUARD_MESSAGE = "network access attempted at measurement time"


class BudgetError(RuntimeError):
    """Raised when an exact budget measurement cannot be produced."""


def _forbid_network(*_args, **_kwargs):
    """Replace ``tiktoken.load.read_file`` so a cache miss can never download.

    ``tiktoken.load.read_file_cached`` resolves ``read_file`` by module-global
    name on every cache miss, so swapping it is enough to block the only
    network path tiktoken has.
    """
    raise RuntimeError(NETWORK_GUARD_MESSAGE)


class OfflineTokenizer:
    """Counts tokens with a verified cl100k_base encoding."""

    def __init__(self, encoding, version: str) -> None:
        self._encoding = encoding
        self._version = version

    @property
    def encoding(self) -> str:
        return ENCODING

    def count(self, text: str) -> int:
        if not text:
            return 0
        return len(self._encoding.encode_ordinary(text))

    def provenance(self) -> dict:
        return {
            "implementation": "tiktoken",
            "version": self._version,
            "encoding": ENCODING,
            "asset_sha256": ASSET_SHA256,
        }


def _read_verified_asset() -> bytes:
    try:
        payload = _ASSET_PATH.read_bytes()
    except FileNotFoundError:
        raise BudgetError(f"missing required encoding asset: {_ASSET_PATH}") from None
    actual = hashlib.sha256(payload).hexdigest()
    if actual != ASSET_SHA256:
        raise BudgetError(
            f"corrupt encoding asset: {_ASSET_PATH} "
            f"(sha256 {actual}, expected {ASSET_SHA256})"
        )
    return payload


def load() -> OfflineTokenizer:
    """Return the verified tokenizer, or raise BudgetError before any measurement."""
    payload = _read_verified_asset()
    try:
        import tiktoken
        import tiktoken.load
    except ImportError:
        raise BudgetError("missing required tokenizer package: tiktoken") from None

    cache_dir = tempfile.mkdtemp(prefix="clio-cl100k-")
    try:
        Path(cache_dir, _CACHE_KEY).write_bytes(payload)
        os.environ["TIKTOKEN_CACHE_DIR"] = cache_dir
        # A wrong cache key must fail loudly instead of falling through to an
        # HTTPS fetch: block tiktoken's only read path for the duration of load.
        original_read_file = tiktoken.load.read_file
        tiktoken.load.read_file = _forbid_network
        try:
            encoding = tiktoken.get_encoding(ENCODING)
        except Exception as exc:  # noqa: BLE001 - any loader failure is fatal here
            raise BudgetError(str(exc)) from exc
        finally:
            tiktoken.load.read_file = original_read_file
        if encoding.name != ENCODING:
            raise BudgetError(
                f"unexpected encoding loaded: {encoding.name!r}, expected {ENCODING!r}"
            )
        version = getattr(tiktoken, "__version__", "unknown")
        return OfflineTokenizer(encoding, version)
    finally:
        shutil.rmtree(cache_dir, ignore_errors=True)
