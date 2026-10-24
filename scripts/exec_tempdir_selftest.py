#!/usr/bin/env python3
# Copyright 2026 Agent Memoir (https://agentmemoir.com/)
# SPDX-License-Identifier: Apache-2.0
"""Self-test: parallel probes of one directory do not collide (issue #323).

Several threads probe the same directory at the same time. A probe that shares
its file name with another thread's probe can be deleted or overwritten by that
thread, and the directory then looks unusable. Every probe must run, and no probe
file may be left behind.
"""

import os
import shutil
import sys
import threading
import traceback
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import exec_tempdir  # noqa: E402 - the helper sits beside this file

THREADS = 8
PROBES_PER_THREAD = 25


def probe_many(
    directory: str, outcomes: list[bool], failures: list[str]
) -> None:
    """Probe one directory repeatedly, recording each answer and failure detail."""
    for i in range(PROBES_PER_THREAD):
        details: list[str] = []
        try:
            ok = exec_tempdir.can_execute(directory, details=details)
            outcomes.append(ok)
            if not ok:
                failures.append(
                    f"thread {threading.get_ident()} probe #{i} returned False: {'; '.join(details)}"
                )
        except Exception as exc:  # noqa: BLE001
            outcomes.append(False)
            failures.append(
                f"thread {threading.get_ident()} probe #{i} raised {type(exc).__name__}: {exc}\n"
                f"{traceback.format_exc()}"
            )


def main() -> int:
    directory = exec_tempdir.make_exec_dir()
    if not directory:
        print("exec_tempdir self-test: skip, no directory on this host permits running a file")
        return 0
    try:
        groups: list[list[bool]] = [[] for _ in range(THREADS)]
        failures: list[list[str]] = [[] for _ in range(THREADS)]
        threads = [
            threading.Thread(target=probe_many, args=(directory, group, fail))
            for group, fail in zip(groups, failures)
        ]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()
        answers = [answer for group in groups for answer in group]
        all_failures = [err for fail in failures for err in fail]
        leftover = sorted(name for name in os.listdir(directory) if name.startswith("_probe"))
        failed = answers.count(False)
        if failed or leftover or len(answers) != THREADS * PROBES_PER_THREAD or all_failures:
            print(
                f"exec_tempdir self-test: FAIL, {failed} of {len(answers)} parallel "
                f"probes said no; leftover probe files: {leftover}"
            )
            if all_failures:
                print("exec_tempdir self-test probe failures:")
                for err in all_failures:
                    print(f"  {err}")
            return 1
        print(f"exec_tempdir self-test: pass, {len(answers)} parallel probes")
        return 0
    finally:
        shutil.rmtree(directory, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
