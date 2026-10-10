#!/usr/bin/env python3
"""Content-addressed gate for heavy cargo and make commands in this repo.

Every workflow stage used to re-run the same suites on the same tree. This
wrapper fingerprints the working tree (HEAD and the content of each changed
path, relevant environment, command kind, effective command), runs the command at most
once per fingerprint, serialises concurrent runs with a lock, and stores the
verdict (exit code and full log) for both pass and fail. A repeated command on an
unchanged tree returns the stored verdict instead of running again. A stored
failure is replayed too, so retrying the same tree does not reroll a flake;
a failure stored under a different PATH is re-run, so a broken environment
does not poison the tree. Edit the tree or pass --force for a fresh run. The
cache keeps the newest 50 verdicts per repository.

Every verdict is published under the name of the kind it was measured as, so a
log names the command that produced it. `cargo nextest` subcommands other than
`run` are stored and published as `nextest-lookup`, each derived coverage target
as itself, and a `cargo llvm-cov` line that measures nothing (a version probe,
`show-env`, `clean`) as `llvm-cov-lookup`. That is what stops a listing, a probe
or a derived report from standing in for a test run or a coverage report in the
log directory.

Quick scoping: `cargo test`, `cargo nextest run`, `cargo clippy`, and the
`make test`, `make check`, `make lint` targets are scoped to the changed
workspace crates and their reverse dependencies (make targets through
`PACKAGES`). Coverage runs are never scoped. A scoped result is labelled quick;
it is not a full-workspace verdict.

There is no full-workspace gate any more. `make coverage` and `cargo llvm-cov`
still produce a whole-workspace report and this wrapper still caches them, but
they judge nothing: the one coverage requirement is that every Rust file a change
created or modified reaches 80% of its lines, and `make coverage-fast` (its own
kind, so a replay is never labelled as the wider report) is what decides a
change. `make test-fast` is deliberately not routed
through this cache: a stored test verdict would replay a flake's pass, which is
the opposite of what a test gate is for.

The tree is fingerprinted again after the command; if it changed while the
command ran, the result is not cached.

`make -n` and its long forms are not cached. A dry run prints its recipes
instead of running them, so it produces no verdict to store, and storing one
would publish a printed recipe list under a real kind's name. The shim passes
those calls straight through, whether the flag arrives as an argument or through
`MAKEFLAGS`. A caller invoking `gate.py run` directly bypasses that, because the
shim is the only surface that sees the flag before the command runs.

Usage:
  python3 scripts/gate.py run [--force] [--log-dir DIR] -- CMD ARGS...
  python3 scripts/gate.py --self-test

Exit codes:
  the command's own exit code, on a fresh run or a reused verdict
  2   usage error
  70  gate internal error (callers should run the command directly)

Environment:
  GATE_RAW=1   bypass the gate entirely (handled by the shim, not here)
  GATE_FORCE=1 same as --force
  GATE_SCOPE=off  never scope a quick command
  CLIO_GATE_LOG_DIR  if the shim passes it, a copy of the newest log lands
                     there as <kind>.log plus <kind>.exit (used by workflows).
                     The run before the current one is kept beside them as
                     <kind>.prev: a header line naming the exit code that run
                     exited with, then its log.
  XDG_CACHE_HOME  cache root; defaults to ~/.cache
"""

from __future__ import annotations

import contextlib
import fcntl
import hashlib
import io
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path

# The shared rule for a temporary directory whose files the host will run lives
# beside this file. It is put on the path the same way the other suites that call
# it do it, so the import holds however this module is reached: run as a script,
# or imported by something that is not sitting in this directory.
sys.path.insert(0, str(Path(__file__).resolve().parent))

from exec_tempdir import exec_tempdir  # noqa: E402

sys.modules.setdefault("gate", sys.modules[__name__])
from gate_kinds import (
    ANSI_ESCAPE,
    CARGO_KINDS,
    COVERAGE_ARTIFACT,
    COVERAGE_KINDS,
    ENV_KEYS,
    ENV_KEY_EXEMPT,
    EXIT_INTERNAL,
    EXIT_USAGE,
    GATE_SCOPE_DEFAULT,
    GATE_SCOPE_OFF,
    KEY_SCHEME,
    LLVM_COV_KNOWN_SUBCOMMANDS,
    LLVM_COV_LOOKUP_KIND,
    LLVM_COV_PROBE_FLAGS,
    LLVM_COV_REPORTING_SUBCOMMANDS,
    LOG_TAIL_LINES,
    MAKE_KINDS,
    NEXTEST_EXECUTING_SUBCOMMANDS,
    NEXTEST_KNOWN_SUBCOMMANDS,
    NEXTEST_LOOKUP_KIND,
    SCOPED_KINDS,
    USAGE,
)


from gate_tree import (
    GateError,
    Member,
    TreeState,
    _effective_env,
    _git,
    _splitz,
    cache_root,
    entry_dir,
    fingerprint,
    read_tree_state,
    repo_key,
    repo_root,
)


from gate_scope import (
    _reaches_every_crate,
    _scope_make,
    changed_seeds,
    closure,
    scope_command,
    workspace_members,
)


from gate_classify import (
    command_kind,
    llvm_cov_measures_coverage,
    llvm_cov_subcommand_index,
    nextest_subcommand,
    nextest_subcommand_index,
)


from gate_cache import (
    _artifact_rel_paths,
    _blob_name,
    _print_tail,
    _publish_log,
    _prune_cache,
    _purge_coverage_cache,
    _restore_artifacts,
    _reuse,
    _save_artifacts,
    acquire_lock,
)


def run_gate(argv: list[str], force: bool = False, log_dir: str = "") -> int:
    cwd = os.getcwd()
    root = repo_root(cwd)
    if root is None:
        raise GateError("not inside a git checkout")
    lock_fd = acquire_lock(root)
    try:
        state = read_tree_state(root)
        kind = command_kind(argv)
        if Path(argv[0]).name == "cargo" and argv[1:2] == ["nextest"] and not nextest_subcommand(argv):
            # Loud on purpose. A subcommand this file does not recognise is
            # classified as a lookup rather than guessed at, which is the safe
            # answer but quietly loses quick scoping and publishes under a name
            # that may not describe the command. Saying so turns a drift in the
            # tool's own command line into something a reader can see.
            print(
                f"gate: warning: no known cargo nextest subcommand in {shlex.join(argv)}; "
                f"treating it as {kind}, which may not be what it does",
                file=sys.stderr,
            )
        # A command that removes build output must run every time, never replay from a
        # stored verdict: a replay would report success without having done the
        # removal. `make coverage-clean` is named as a target, and `cargo llvm-cov
        # clean` is the same request to the other tool. `clean` is a lookup kind,
        # not a coverage one, so it is not in COVERAGE_KINDS and does not need the
        # artifact handling that set buys.
        clean_requested = "coverage-clean" in argv[1:] or (
            len(argv) > 2 and argv[1:3] == ["llvm-cov", "clean"]
        )
        if clean_requested:
            force = True
        scoped_argv, scope = scope_command(kind, argv, root, state.changed)
        fp = fingerprint(state, os.environ, kind, scoped_argv)
        entry = entry_dir(root, fp)
        meta_path = entry / "meta.json"
        if meta_path.is_file() and not force:
            try:
                meta = json.loads(meta_path.read_text())
            except (OSError, ValueError):
                meta = None
            if isinstance(meta, dict) and meta.get("schema") == 1 and meta.get("tree") == fp:
                fresh_env = meta.get("exit", 1) == 0 or meta.get("path") == os.environ.get("PATH", "")
                if fresh_env:
                    _restore_artifacts(entry, root, meta)
                    return _reuse(entry, meta, log_dir)
                print("gate: stored failure came from a different PATH; re-running", file=sys.stderr)
        entry.mkdir(parents=True, exist_ok=True)
        log_path = entry / "log"
        if scope:
            print(f"gate: quick scoped run ({kind}) on changed crates: {', '.join(scope)}")
            print("gate: not a full-workspace verdict; run `make coverage` before claiming done")
        print(f"gate: running {shlex.join(scoped_argv)}")
        started = time.time()
        env = dict(os.environ)
        env["GATE_ACTIVE"] = "1"
        with open(log_path, "wb") as log:
            code = subprocess.call(scoped_argv, cwd=cwd, env=env, stdout=log, stderr=subprocess.STDOUT)
        finished = time.time()
        published = _publish_log(log_path, log_dir, kind, code)
        if clean_requested:
            _purge_coverage_cache(root)
            print(f"gate: {kind} exit {code} in {round(finished - started, 1)}s; log: {published or log_path}")
            if code != 0:
                _print_tail(log_path)
            return code
        if fingerprint(read_tree_state(root), os.environ, kind, scoped_argv) != fp:
            print("gate: tree changed while the command ran; result not cached", file=sys.stderr)
            print(f"gate: log: {published or log_path}")
            return code
        meta = {
            "schema": 1,
            "tree": fp,
            "kind": kind,
            "argv": scoped_argv,
            "scope": scope,
            "exit": code,
            "path": os.environ.get("PATH", ""),
            "started": started,
            "finished": finished,
            "seconds": round(finished - started, 1),
            "artifacts": _save_artifacts(entry, root, scoped_argv, kind),
            "log": "log",
        }
        tmp = entry / "meta.json.tmp"
        tmp.write_text(json.dumps(meta, indent=2) + "\n")
        os.replace(tmp, meta_path)
        _prune_cache(root)
        print(f"gate: {kind} exit {code} in {meta['seconds']}s; log: {published or log_path}")
        if code != 0:
            _print_tail(log_path)
        return code
    finally:
        os.close(lock_fd)


def _self_test() -> int:
    import unittest

    import gate_selftest_a
    import gate_selftest_b
    import gate_selftest_c

    loader = unittest.TestLoader()
    suite = unittest.TestSuite()
    for mod in (gate_selftest_a, gate_selftest_b, gate_selftest_c):
        suite.addTests(loader.loadTestsFromModule(mod))
    buffer = io.StringIO()
    result = unittest.TextTestRunner(stream=buffer, verbosity=1).run(suite)
    skipped = [f"{test}: {reason}" for test, reason in getattr(result, "skipped", [])]
    if not result.wasSuccessful():
        print(buffer.getvalue())
    for reason in skipped:
        print(f"self-test skipped: {reason}")
    clean = result.wasSuccessful()
    if not clean:
        summary = "FAILED"
    elif skipped:
        summary = f"all passed, {len(skipped)} check group(s) skipped"
    else:
        summary = "all passed"
    print(f"self-test: {summary}")
    return 0 if clean else 1


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return _self_test()
    if "-h" in argv or "--help" in argv:
        print(__doc__.strip())
        return 0
    if not argv or argv[0] != "run":
        print(USAGE, file=sys.stderr)
        return EXIT_USAGE
    rest = argv[1:]
    if "--" not in rest:
        print(USAGE, file=sys.stderr)
        return EXIT_USAGE
    sep = rest.index("--")
    head = list(rest[:sep])
    force = "--force" in head or os.environ.get("GATE_FORCE") == "1"
    log_dir = ""
    while "--log-dir" in head:
        index = head.index("--log-dir")
        if index + 1 >= len(head):
            print(USAGE, file=sys.stderr)
            return EXIT_USAGE
        log_dir = head[index + 1]
        del head[index : index + 2]
    command = rest[sep + 1 :]
    if not command:
        print(USAGE, file=sys.stderr)
        return EXIT_USAGE
    try:
        return run_gate(command, force=force, log_dir=log_dir)
    except GateError as e:
        print(f"gate: error: {e}", file=sys.stderr)
        return EXIT_INTERNAL
    except Exception as e:  # noqa: BLE001 - never fail silently; the shim falls back
        print(f"gate: internal error: {e!r}", file=sys.stderr)
        return EXIT_INTERNAL


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
