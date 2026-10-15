#!/usr/bin/env python3
"""Read kernel counters for a process tree from the host.

`ps` provides the per-process counters sampled while the child runs, and the OS accounting API provides the exact end-of-run user/system split `ps` cannot.
This module owns the host probes, the `ps` keyword mapping, and the process-tree walk; the sampling loop itself lives in `phase_timer`.
"""

from __future__ import annotations

import os
import subprocess

from phase_model import COUNTER_FIELDS


# How often to read the kernel's counters for the child's process tree. One
# second is fine enough to attribute a multi-second stall to the interval it
# happened in, and the cost is a handful of `ps` calls per tick.
SAMPLE_INTERVAL_SECONDS = 1.0


def _read_counters(pid: int) -> tuple[int, dict[str, float]] | None:
    """Read one process's kernel counters, or None if it is gone.

    `ps` is used rather than /proc because this runs on macOS, where /proc does
    not exist. Only the fields this report needs are requested, so the cost per
    sample is one fork per live process at the sampling interval.
    """
    # `readb`, `writeb`, and `rsspeak` are Linux-only ps keywords and are absent
    # on macOS, where asking for them makes ps print a keyword error and no
    # counters at all. Probe the supported set once, then request only those, so
    # one unsupported field cannot silently zero out every measurement.
    global _PS_FIELDS  # noqa: PLW0603
    if _PS_FIELDS is None:
        _PS_FIELDS = _supported_ps_fields()
    if not _PS_FIELDS:
        return None
    done = subprocess.run(  # noqa: S603
        ["ps", "-o", f"{','.join(_PS_FIELDS)}=", "-p", str(pid)],  # noqa: S607
        capture_output=True,
        text=True,
        check=False,
    )
    if done.returncode != 0:
        return None
    # ps prints a header row even with the `=` suffix, and prints `-` for a
    # counter it cannot collect on this host or for this process. Both are
    # normal, not errors: take the data row, which is the last line whose first
    # token is a number, and read `-` as a zero for that one field rather than
    # discarding the whole reading. Selecting by content rather than position
    # matters because ps right-aligns columns and collapses headers, so token
    # counts do not line up with the requested field count.
    parts: list[str] = []
    for row in reversed(done.stdout.splitlines()):
        tokens = row.split()
        if tokens and _is_number(tokens[0]):
            parts = tokens
            break
    if not parts:
        return None
    # The `time` column is [[dd-]hh:]mm:ss rather than a number, so it is kept
    # as its own string and converted once, below, where it is used.
    time_token = "0"
    values: list[float] = []
    for name, token in zip(_PS_FIELDS, parts[: len(_PS_FIELDS)], strict=False):
        if token == "-":
            values.append(0.0)
        elif name == "time":
            time_token = token
            values.append(0.0)
        else:
            values.append(float(token))
    if len(values) != len(_PS_FIELDS):
        return None
    # `ps` keyword -> this report's counter name. They differ: ps calls resident
    # size `rss` and the fault counts `minflt`/`majflt`, while the report groups
    # them under the names the OS accounting API uses. Reading the ps names
    # straight into the report would file every one of them under a key the
    # report never looks at, so the counters would all read zero.
    reading = {name: 0.0 for name in COUNTER_FIELDS}
    for name, value in zip(_PS_FIELDS, values, strict=True):
        target = PS_TO_COUNTER.get(name)
        if target is not None:
            reading[target] = value
    # macOS `ps` reports one combined time column with no user/system split, so
    # it is charged to system time here and the exact end-of-run figures from the
    # OS accounting API correct the split afterwards. Charging it to system time
    # is the conservative direction: it can overstate kernel cost, never hide it.
    reading["system_seconds"] = _ps_time_seconds(time_token)
    reading["user_seconds"] = 0.0
    return 1, reading


# `ps` keyword -> this report's counter name. They differ: ps calls resident size
# `rss` and the fault counts `minflt`/`majflt`, while the report groups them under
# the names the OS accounting API uses. Without this map every sampled counter
# would be filed under a key the report never reads, and all of them would read
# zero — a measurement that looks valid and says nothing.
PS_TO_COUNTER = {
    "rss": "max_rss_kb",
    "rsspeak": "peak_rss_kb",
    "time": "time",
    "minflt": "minor_faults",
    "majflt": "major_faults",
    "nvcsw": "voluntary_ctx_switches",
    "nivcsw": "involuntary_ctx_switches",
    "readb": "read_bytes",
    "writeb": "write_bytes",
}


_PS_FIELDS: list[str] | None = None


def _is_number(text: str) -> bool:
    try:
        float(text)
    except ValueError:
        return False
    return True


def _supported_ps_fields() -> list[str]:
    """The counter keywords this host's `ps` actually understands.

    A keyword counts as supported only when the probe exits zero. The exit
    status is the signal; the wording on stderr is not. `ps` refuses an unknown
    keyword differently on each platform — macOS says ``readb: keyword not
    found``, procps says ``unknown user-defined format specifier "readb"``, and
    toybox has its own phrasing again — so matching on the message marks a
    keyword supported on a host that cannot print it, and one unsupported field
    then fails every combined read and silently zeroes the whole run's sampling.

    The probe asks about this process rather than pid 1 so a "no such process"
    failure cannot be mistaken for an unsupported keyword: a live pid is a
    precondition for the exit status to mean what it is read to mean.
    """
    wanted = [
        "rss",
        "time",
        "minflt",
        "majflt",
        "nvcsw",
        "nivcsw",
        "readb",
        "writeb",
        "rsspeak",
    ]
    supported: list[str] = []
    for name in wanted:
        probe = subprocess.run(  # noqa: S603
            ["ps", "-o", f"{name}=", "-p", str(os.getpid())],  # noqa: S607
            capture_output=True,
            text=True,
            check=False,
        )
        if probe.returncode == 0:
            supported.append(name)
    return supported


def _ps_time_seconds(field: str) -> float:
    """`ps` time is [[dd-]hh:]mm:ss, all of it charged to user plus system.

    macOS `ps` does not split user from system in the `time` column, so this
    returns the combined figure. That is enough to answer the question the
    report exists to answer (how much was the kernel versus the CPU) at the
    process-tree level, where `times()` provides the split.
    """
    days = 0
    if "-" in field:
        day_part, field = field.split("-", 1)
        days = int(day_part)
    chunks = field.split(":")
    seconds = 0.0
    for chunk in chunks:
        seconds = seconds * 60 + float(chunk)
    return seconds + days * 86400


def _descendants(root: int) -> list[int]:
    """The root pid plus every live descendant, via one `pgrep -P` walk."""
    found = [root]
    frontier = [root]
    while frontier:
        done = subprocess.run(  # noqa: S603
            ["pgrep", "-P", str(frontier.pop())],  # noqa: S607
            capture_output=True,
            text=True,
            check=False,
        )
        for token in done.stdout.split():
            try:
                child = int(token)
            except ValueError:
                continue
            if child not in found:
                found.append(child)
                frontier.append(child)
    return found


def _accurate_tree_counters(root: int) -> dict[str, float] | None:
    """Exact user/system split for the tree, from the OS accounting API.

    `ps` cannot split user from system, and that split is the whole question: a
    build can spend three times its CPU time inside the kernel and still print a
    normal-looking log. Falls back to None when `resource` is unavailable, which
    is the path the report names rather than hides.
    """
    try:
        import resource
    except ImportError:  # pragma: no cover - not reachable on macOS or Linux
        return None
    try:
        usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    except (OSError, ValueError):
        return None
    return {
        "user_seconds": float(usage.ru_utime),
        "system_seconds": float(usage.ru_stime),
        "max_rss_kb": float(usage.ru_maxrss),
        "minor_faults": float(usage.ru_minflt),
        "major_faults": float(usage.ru_majflt),
        "voluntary_ctx_switches": float(usage.ru_nvcsw),
        "involuntary_ctx_switches": float(usage.ru_nivcsw),
        "read_bytes": 0.0,
        "write_bytes": 0.0,
        "peak_rss_kb": float(usage.ru_maxrss),
    }
