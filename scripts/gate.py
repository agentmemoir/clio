#!/usr/bin/env python3
"""Content-addressed gate for heavy cargo and make commands in this repo.

Each cached command is keyed by the changed tree, relevant environment, kind, and effective arguments; it runs once per fingerprint and replays its pass or failure until the tree changes or `--force` is used. A failure from a different `PATH` is rerun.

The cache holds up to 50 entries per worktree and removes lookup entries first. Every run and replay is appended to `runs.jsonl` beside the cache with its worktree, branch, tree fingerprint and machine load, so gate wait time can be measured after pruning.

Each published log names the command kind and starts with a fresh-run or replay marker. Cargo check, clippy, lint, and ritual logs report package work Cargo showed, or say when its status was absent. Nextest and llvm-cov lookups publish to separate log names without using cache verdict slots; nextest benchmarks have their own cached kind.

A published log also carries a `gate: run ...` line, naming the worktree, the tree, and the finish time of the run the record describes, and the published exit file carries that same line after its exit code. The publish holds one lock on the published directory, writes the exit code first, and puts the verdict back when the log cannot be written, so a reader never finds one run's verdict beside another run's log. All of it is needed because a published directory can be shared by every worktree of a checkout.

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
change. `make test-fast`, `make precommit` and `make quick` are deliberately not
cached, even behind another goal on the same line: a stored test verdict would
replay a flake's pass, which is the opposite of what a test gate is for.

The tree is fingerprinted again after the command; if it changed while the
command ran, the result is not cached.

`make -n` and its long forms are not cached. A dry run prints recipes instead of running them, so it produces no verdict to store; both the shim and direct `gate.py run` calls use this module's check.

Usage:
  python3 scripts/gate.py run [--force] [--log-dir DIR] -- CMD ARGS...
  python3 scripts/gate.py --self-test

Exit codes:
  the command's own exit code, on a fresh run or a reused verdict
  2   usage error
  3   refused: a run with no terminal attached of `make coverage-fast` that the
      Done check rules out (it ran before on this branch, or it is estimated
      over its budget, or it cannot be estimated); nothing ran
  70  gate internal error (callers should run the command directly)

Environment:
  GATE_RAW=1   bypass the gate entirely (handled by the shim, not here)
  GATE_FORCE=1 same as --force
  GATE_SCOPE=off  never scope a quick command
  CLIO_GOLDEN_CAPTURE=1  the golden capture runs every time and stores no
                     verdict; a stored one would replay a refusal after the
                     cache it asked for was warmed
  CLIO_GATE_LOG_DIR  optional log directory override. By default, logs land
                     under this repository's clio-gate cache folder. Each
                     <kind>.log starts with a fresh-run or replay marker and
                     names the run it records; each <kind>.exit holds the exit
                     code followed by that same run line. The previous log is
                     kept beside them as <kind>.prev.
  XDG_CACHE_HOME  cache root; defaults to ~/.cache
"""

from __future__ import annotations

import contextlib
import fcntl
import hashlib
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
from datetime import datetime, timezone
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
    LOOKUP_KINDS,
    MAKE_KINDS,
    UNCACHED_MAKE_GOALS,
    NEXTEST_EXECUTING_SUBCOMMANDS,
    NEXTEST_KNOWN_SUBCOMMANDS,
    NEXTEST_LOOKUP_KIND,
    RUN_RECORD_PREFIX,
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
    make_is_dry_run,
    make_target_indexes,
    nextest_subcommand,
    nextest_subcommand_index,
)


from gate_runlog import append_run, machine_load
from gate_done_check import EXIT_REFUSED, decide as done_check
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
    if make_is_dry_run(argv, os.environ):
        env = dict(os.environ)
        env["GATE_ACTIVE"] = "1"
        return subprocess.call(argv, cwd=cwd, env=env)
    root = repo_root(cwd)
    if root is None:
        raise GateError("not inside a git checkout")
    lock_fd = acquire_lock(root)
    ran: dict[str, object] = {"replayed": False, "load": machine_load()}
    try:
        log_dir = log_dir or str(cache_root() / repo_key(root) / "logs")
        kind = command_kind(argv)
        ran.update(kind=kind, argv=argv)
        clean_requested = "coverage-clean" in argv[1:] or (
            len(argv) > 2 and argv[1:3] == ["llvm-cov", "clean"]
        )
        # A golden capture writes the frozen CLI fixture from a guarded release
        # build, so whether it can proceed depends on the release build cache,
        # which is not part of the fingerprint: a warm cache changes nothing the
        # key holds. A stored refusal would therefore replay after the step its
        # own refusal message asks for -- warming that cache -- which is the
        # defect #136 records, and a stored success would let a later run report
        # a capture it never made. The capture is a must-run command, and the
        # early return below is what keeps this run's verdict out of the cache.
        capture_requested = os.environ.get("CLIO_GOLDEN_CAPTURE") == "1"
        # Make runs every goal on its line, so `make lint test-fast` runs the
        # tests too and must never replay a pass stored for the whole line.
        uncached_goal = Path(argv[0]).name == "make" and any(
            argv[index] in UNCACHED_MAKE_GOALS for index in make_target_indexes(argv)
        )
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
        if kind in LOOKUP_KINDS:
            with tempfile.TemporaryDirectory(
                prefix="lookup-", dir=cache_root() / repo_key(root)
            ) as scratch:
                log_path = Path(scratch) / "log"
                print(f"gate: running uncached lookup ({kind}) {shlex.join(argv)}")
                started = time.time()
                env = dict(os.environ)
                env["GATE_ACTIVE"] = "1"
                with open(log_path, "wb") as log:
                    code = subprocess.call(argv, cwd=cwd, env=env, stdout=log, stderr=subprocess.STDOUT)
                finished = time.time()
                ran.update(started=started, finished=finished, exit=code)
                published = _publish_log(log_path, log_dir, kind, code, run_line=_run_line(root, "", finished))
                if clean_requested:
                    _purge_coverage_cache(root)
                print(f"gate: {kind} exit {code}; log: {published or log_path}")
                if code != 0:
                    _print_tail(log_path)
                return code
        state = read_tree_state(root)
        # A command that removes build output must run every time, never replay from a
        # stored verdict: a replay would report success without having done the
        # removal. `make coverage-clean` is named as a target, and `cargo llvm-cov
        # clean` is the same request to the other tool. `clean` is a lookup kind,
        # not a coverage one, so it is not in COVERAGE_KINDS and does not need the
        # artifact handling that set buys.
        #
        # A golden capture is a must-run command for the same kind of reason: it
        # may only proceed once the release build cache is warm, which the key
        # cannot see, so a replayed verdict is always about a different cache
        # state than the one the run meets.
        if clean_requested or capture_requested or uncached_goal:
            force = True
        scoped_argv, scope = scope_command(kind, argv, root, state.changed)
        ran.update(argv=scoped_argv)
        fp = fingerprint(state, os.environ, kind, scoped_argv)
        ran["tree"] = fp
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
                    replay_started = time.time()
                    _restore_artifacts(entry, root, meta)
                    run_line = _run_line(root, str(meta.get("tree", "")), meta.get("finished"))
                    code = _reuse(entry, meta, log_dir, run_line=run_line)
                    ran.update(replayed=True, started=replay_started, finished=time.time(), exit=code)
                    return code
                print("gate: stored failure came from a different PATH; re-running", file=sys.stderr)
        decision = done_check(kind, scoped_argv, root)
        if decision.reason:
            print(f"gate: {kind} refused: {decision.reason}", file=sys.stderr)
            return EXIT_REFUSED
        if decision.estimate_s is not None:
            ran["estimate_s"] = decision.estimate_s
        entry.mkdir(parents=True, exist_ok=True)
        log_path = entry / "log"
        if scope:
            print(f"gate: quick scoped run ({kind}) on changed crates: {', '.join(scope)}")
            print("gate: not a full-workspace verdict; CI owns that verdict")
        print(f"gate: running {shlex.join(scoped_argv)}")
        started = time.time()
        env = dict(os.environ)
        env["GATE_ACTIVE"] = "1"
        with open(log_path, "wb") as log:
            code = subprocess.call(scoped_argv, cwd=cwd, env=env, stdout=log, stderr=subprocess.STDOUT)
        finished = time.time()
        ran.update(started=started, finished=finished, exit=code)
        try:
            evidence = _cargo_checking_evidence(log_path, kind)
        except OSError as error:
            evidence = b""
            print(f"gate: warning: cannot read Cargo work evidence: {error}", file=sys.stderr)
        if evidence:
            try:
                with log_path.open("ab") as output:
                    output.write(b"\n" + evidence)
            except OSError as error:
                print(f"gate: warning: cannot record Cargo work evidence: {error}", file=sys.stderr)
        published = _publish_log(log_path, log_dir, kind, code, run_line=_run_line(root, fp, finished))
        # Every must-run command returns here, before the verdict is stored: a
        # clean stores nothing so its removal is never replayed, a capture
        # stores nothing so a later run of the same command cannot inherit a
        # refusal measured against a colder cache, and a line with a never-cached
        # goal stores nothing so a test pass is never replayed.
        if clean_requested or capture_requested or uncached_goal:
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
        if "exit" in ran:
            append_run(root, ran)
        os.close(lock_fd)


def _run_line(root: str, tree: str, finished: float | None) -> bytes:
    """One line naming the run a published record came from.

    A published directory can be shared by every worktree of one checkout, because
    `CLIO_GATE_LOG_DIR` sends them all to one directory so a reader can find the
    newest verdict. The record alone then cannot say whose run it describes, which
    is the first half of issue #186: a verdict published by another worktree's
    zero-work replay read as the reader's own. The line names the worktree, the
    tree the verdict was measured from, and when that run finished, so a reader can
    tell a foreign or older verdict from their own without reading the cache.
    """
    when = "?"
    if finished:
        when = datetime.fromtimestamp(finished, tz=timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    return f"{RUN_RECORD_PREFIX.decode()}worktree={root} tree={tree[:12] or '-'} finished={when}\n".encode()


def _cargo_checking_evidence(log_path: Path, kind: str) -> bytes:
    if kind not in {"check", "clippy", "lint", "ritual"}:
        return b""
    checking: set[tuple[str, str]] = set()
    compiling: set[tuple[str, str]] = set()
    finished = False
    with log_path.open("r", errors="replace") as output:
        for line in output:
            line = ANSI_ESCAPE.sub("", line)
            finished = finished or re.match(r"^\s*Finished\b", line) is not None
            match = re.match(
                r"^\s*(Checking|Compiling)\s+([^\s]+)\s+v([^\s]+)",
                line,
            )
            if match:
                packages = checking if match.group(1) == "Checking" else compiling
                packages.add((match.group(2), match.group(3)))
    if not checking and not compiling and not finished:
        return b"gate: Cargo package work: unavailable (no status lines)\n"
    return f"gate: Cargo package work: {len(checking)} checked, {len(compiling)} compiled\n".encode()


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        from gate_selftest_suite import run_suite

        return run_suite()
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
    log_dir = os.environ.get("CLIO_GATE_LOG_DIR", "")
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
