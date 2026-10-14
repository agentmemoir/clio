from __future__ import annotations

import contextlib
import fcntl
import hashlib
import itertools
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
    RUN_RECORD_PREFIX,
)
from gate_tree import cache_root, repo_key

#: The lock a publisher holds on the published directory itself, beside the files
#: it protects. It is the directory, not the worktree, that decides who contends
#: for it, because that is what two worktrees can share.
PUBLISH_LOCK_NAME = ".gate-publish.lock"
#: Makes each staging name unique inside its own process, so two publishers
#: cannot pick the same one.
_STAGING = itertools.count()


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


def _replace_bytes(path: Path, data: bytes) -> None:
    """Put ``data`` at ``path`` in one step, so a reader sees one run's file or the other's.

    `rename(2)` replaces an existing destination atomically: "there is no point at
    which another process attempting to access newpath will find it missing", and a
    reader of the new name sees "either ... the file referred to by new or old
    before the operation began". A plain truncate-and-write has neither property:
    a reader can catch the file empty, which is what a verdict check reads as no
    verdict at all.

    The staging name carries the process id and a counter because two publishers
    can share one directory. A staging name they both pick is how one publisher's
    cleanup removed another's file, which the gate printed as a
    ``FileNotFoundError`` on ``<kind>.prev.staged``.
    """
    staged = path.with_name(f".{path.name}.{os.getpid()}.{next(_STAGING)}.staged")
    try:
        staged.write_bytes(data)
        os.replace(staged, path)
    finally:
        staged.unlink(missing_ok=True)


def _body_only(published: bytes) -> bytes:
    """A published record without its run line, which names the run and so changes every run.

    The store already holds one rule about identical output: identical bytes are
    not a new run, because inventing a false earlier run is the worse error for an
    evidence store. Comparing the body alone keeps that rule while the record names
    the run that produced it.
    """
    marker = REPLAYED_LOG_MARKER if published.startswith(REPLAYED_LOG_MARKER) else EXECUTED_LOG_MARKER
    rest = published[len(marker) :]
    if rest.startswith(RUN_RECORD_PREFIX):
        _line, separator, tail = rest.partition(b"\n")
        if separator:
            rest = tail
    return marker + rest


@contextlib.contextmanager
def _publish_lock(dest_dir: Path):
    """Hold an exclusive lock on a published directory for as long as a publish takes.

    The gate's own lock is per worktree, because it guards that worktree's build
    directory. A published directory is not per worktree: `CLIO_GATE_LOG_DIR`
    points every worktree of one checkout at one directory, which is what lets a
    reader find the newest verdict. Nothing ordered two such worktrees, so their
    publishes interleaved and the log and the exit file could come from two
    different runs.

    `flock(2)` gives the exclusion: "Only one process may hold an exclusive lock
    for a given file at a given time". It is an advisory lock, which is enough
    because every writer here takes it. It is released when the descriptor closes.
    """
    lock_path = dest_dir / PUBLISH_LOCK_NAME
    descriptor = os.open(lock_path, os.O_CREAT | os.O_RDWR, 0o644)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        yield
    finally:
        os.close(descriptor)


def _publish_log(
    log_path: Path, log_dir: str, kind: str, code: int, replayed: bool = False, run_line: bytes = b""
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

    The whole publish holds one lock on the directory, and each file is replaced in
    a single step. The pair needs both, because two worktrees can publish into one
    directory and the gate's own lock is per worktree. The exit code is written
    first, and it is the verdict every reader trusts: a run that cannot write it
    publishes nothing, so an older exit code is never left beside a newer log.
    That mismatch is what issue #186 records -- a published exit file still
    describing an earlier run after a failing run and again after a later green
    one. A reader can still catch a new exit code beside the previous log in the
    moment between the two renames; the verdict is never the stale half.
    """
    if not log_dir:
        return None
    try:
        dest_dir = Path(log_dir)
        dest_dir.mkdir(parents=True, exist_ok=True)
        dest = dest_dir / f"{kind}.log"
        exit_file = dest_dir / f"{kind}.exit"
        marker = REPLAYED_LOG_MARKER if replayed else EXECUTED_LOG_MARKER
        log_body = log_path.read_bytes()
        body = marker + log_body
        record = marker + run_line + log_body
        with _publish_lock(dest_dir):
            existing = b""
            if dest.is_file():
                try:
                    existing = dest.read_bytes()
                except OSError:
                    existing = b""
            try:
                carried = exit_file.read_text().strip() or "unknown"
            except OSError:
                carried = "unknown"
            _replace_bytes(exit_file, f"{code}\n".encode())
            if existing and _body_only(existing) != body and not replayed:
                _replace_bytes(dest_dir / f"{kind}.prev", f"exit: {carried}\n".encode() + existing)
            _replace_bytes(dest, record)
        return dest
    except OSError as e:
        print(f"gate: warning: cannot publish log to {log_dir}: {e}", file=sys.stderr)
        return None


def _reuse(entry: Path, meta: dict, log_dir: str, run_line: bytes = b"") -> int:
    code = int(meta["exit"])
    print(
        f"gate: reusing stored {meta['kind']} verdict for tree {meta['tree'][:12]}: "
        f"exit {code} ({meta.get('seconds', '?')}s)"
    )
    log_path = entry / meta.get("log", "log")
    published = _publish_log(log_path, log_dir, meta["kind"], code, replayed=True, run_line=run_line)
    print(f"gate: log: {published or log_path}")
    if code != 0:
        _print_tail(log_path)
    return code
