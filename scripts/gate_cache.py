from __future__ import annotations

import fcntl
import hashlib
import json
import os
import shutil
import sys
from pathlib import Path

from gate_kinds import (
    COVERAGE_ARTIFACT,
    COVERAGE_KINDS,
    EXECUTED_LOG_MARKER,
    LOOKUP_KINDS,
    LOG_TAIL_LINES,
    REPLAYED_LOG_MARKER,
)
from gate_tree import cache_root, repo_key

def acquire_lock(root: str) -> int:
    lock_path = cache_root() / repo_key(root) / "lock"
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(lock_path, os.O_CREAT | os.O_RDWR, 0o644)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        print("gate: another run holds the lock; waiting...", file=sys.stderr)
        fcntl.flock(fd, fcntl.LOCK_EX)
    return fd


def _artifact_rel_paths(argv: list[str], kind: str) -> list[str]:
    found: list[str] = []
    want_next = False
    for arg in argv:
        if want_next:
            found.append(arg)
            want_next = False
        elif arg == "--output-path":
            want_next = True
        elif arg.startswith("--output-path="):
            found.append(arg.split("=", 1)[1])
    if kind in COVERAGE_KINDS:
        found.append(COVERAGE_ARTIFACT)
    return list(dict.fromkeys(found))


def _blob_name(rel: str) -> str:
    return hashlib.sha256(rel.encode()).hexdigest()[:12] + "-" + Path(rel).name


def _save_artifacts(entry: Path, root: str, argv: list[str], kind: str) -> list[str]:
    saved = []
    for rel in _artifact_rel_paths(argv, kind):
        src = Path(root) / rel
        if not src.is_file():
            continue
        dest = entry / "artifacts" / _blob_name(rel)
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(src.read_bytes())
        saved.append(rel)
    return saved


def _restore_artifacts(entry: Path, root: str, meta: dict) -> None:
    """Put back an artifact a replay would otherwise leave missing.

    A replay does not run the command, so a file the command wrote is not written
    again, and this is what puts it back.

    It never replaces a file that is already there. The copy in the cache is a
    snapshot taken at the end of an earlier run, while a report lives under
    `target/` and so is not part of the fingerprint a replay matches on. Writing
    the older copy over a newer one would leave the reader holding a measurement
    of a different tree, with a green verdict beside it and nothing to say the two
    do not match. An empty live file is the one exception: it holds no
    measurement at all, which is what a shell redirect or an interrupted write
    leaves behind, so restoring over it is a repair.
    """
    for rel in meta.get("artifacts", []):
        src = entry / "artifacts" / _blob_name(rel)
        if not src.is_file():
            continue
        live = Path(root) / rel
        if live.is_file() and live.stat().st_size:
            print(f"gate: kept {rel}; a file is already there and this replay measured nothing")
            continue
        live.parent.mkdir(parents=True, exist_ok=True)
        live.write_bytes(src.read_bytes())
        print(f"gate: restored {rel} from cache")


def _purge_coverage_cache(root: str) -> None:
    repo_cache = cache_root() / repo_key(root)
    if not repo_cache.is_dir():
        return
    removed = 0
    for meta_path in repo_cache.glob("*/meta.json"):
        try:
            meta = json.loads(meta_path.read_text())
        except (OSError, ValueError):
            continue
        if isinstance(meta, dict) and meta.get("kind") in COVERAGE_KINDS:
            shutil.rmtree(meta_path.parent, ignore_errors=True)
            removed += 1
    if removed:
        print(f"gate: cleared {removed} cached coverage verdict(s) after coverage-clean")


def _prune_cache(root: str, keep: int = 50) -> None:
    repo_cache = cache_root() / repo_key(root)
    if not repo_cache.is_dir():
        return
    metas = []
    for path in repo_cache.glob("*/meta.json"):
        try:
            kind = json.loads(path.read_text()).get("kind")
        except (OSError, ValueError, AttributeError):
            kind = None
        metas.append((kind in LOOKUP_KINDS, -path.stat().st_mtime, path.parent))
    metas.sort()
    for _lookup, _newest, stale in metas[keep:]:
        shutil.rmtree(stale, ignore_errors=True)


def _print_tail(log_path: Path) -> None:
    try:
        lines = log_path.read_text(errors="replace").splitlines()
    except OSError:
        return
    shown = lines[-LOG_TAIL_LINES:]
    print(f"gate: last {len(shown)} log lines:")
    for line in shown:
        print(f"gate| {line}")


def _publish_log(
    log_path: Path, log_dir: str, kind: str, code: int, replayed: bool = False
) -> Path | None:
    """Publish the newest run's log and keep the run it replaces.

    Three files, and only the first two existed before:

    * ``<kind>.log`` and ``<kind>.exit`` -- the newest run, under the names every
      reader and every recorded citation already uses.
    * ``<kind>.prev`` -- the run before it, as **one** file: a header line naming
      the exit code that run exited with, then its log.

    The kept run is one file rather than a pair because two files cannot be
    swapped without a window in which the log and the exit code describe different
    runs, and a record carrying the wrong exit code is worse than no record. One
    file written to a staging path and renamed into place has no such window: the
    rename is the only step that becomes visible.

    A file already at the published name is kept only when it is non-empty and
    different. An empty one is what a shell redirect leaves behind, so it is the
    common case and not a corner: preserving it would overwrite the previous
    run's record with nothing and lose the evidence this function exists to keep.
    A replay adds an explicit marker to the published log and does not rotate `<kind>.prev`; the cached command output stays unchanged. Identical fresh logs are not separated, because inventing a false earlier run is the worse error for an evidence store.

    A log that cannot be read is not overwritten on the strength of a failure to
    read it: the read is guarded separately so an unreadable leftover costs the
    kept run and nothing else.
    """
    if not log_dir:
        return None
    try:
        dest_dir = Path(log_dir)
        dest_dir.mkdir(parents=True, exist_ok=True)
        dest = dest_dir / f"{kind}.log"
        exit_file = dest_dir / f"{kind}.exit"
        marker = REPLAYED_LOG_MARKER if replayed else EXECUTED_LOG_MARKER
        fresh = marker + log_path.read_bytes()
        existing = b""
        if dest.is_file():
            try:
                existing = dest.read_bytes()
            except OSError:
                existing = b""
        if existing and existing != fresh and not replayed:
            try:
                carried = exit_file.read_text().strip() or "unknown"
            except OSError:
                carried = "unknown"
            staged = dest_dir / f".{kind}.prev.staged"
            try:
                staged.write_bytes(f"exit: {carried}\n".encode() + existing)
                staged.replace(dest_dir / f"{kind}.prev")
            finally:
                staged.unlink(missing_ok=True)
        dest.write_bytes(fresh)
        exit_file.write_text(f"{code}\n")
        return dest
    except OSError as e:
        print(f"gate: warning: cannot publish log to {log_dir}: {e}", file=sys.stderr)
        return None


def _reuse(entry: Path, meta: dict, log_dir: str) -> int:
    code = int(meta["exit"])
    print(
        f"gate: reusing stored {meta['kind']} verdict for tree {meta['tree'][:12]}: "
        f"exit {code} ({meta.get('seconds', '?')}s)"
    )
    log_path = entry / meta.get("log", "log")
    published = _publish_log(log_path, log_dir, meta["kind"], code, replayed=True)
    print(f"gate: log: {published or log_path}")
    if code != 0:
        _print_tail(log_path)
    return code
