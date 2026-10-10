"""Compare the settings of two am-bench reports.

This module compares the `systems.<system>.settings` block of two am-bench
reports. It separates a change in the named profile setting from any other
change. The comparison is over parsed JSON values.
"""

PROFILE_SETTING = "retention_profile"


def settings_of(report: dict, system: str = "clio") -> dict:
    """Return `report["systems"][system]["settings"]`."""
    try:
        systems = report["systems"]
    except (KeyError, TypeError):
        raise KeyError(f"report has no 'systems' block: {report!r}")
    try:
        system_block = systems[system]
    except (KeyError, TypeError):
        raise KeyError(f"report has no system {system!r}")
    try:
        return system_block["settings"]
    except (KeyError, TypeError):
        raise KeyError(f"system {system!r} has no 'settings' block")


def changed_settings(a: dict, b: dict) -> dict:
    """Return every setting whose value differs between `a` and `b`.

    The result is keyed by setting name (sorted). Each value is
    `{"a": <value_a or None>, "b": <value_b or None>}`. A name that is
    present on only one side counts as changed, and its missing side is
    `None`. Equal names are left out.
    """
    changed = {}
    for name in sorted(set(a) | set(b)):
        value_a = a.get(name)
        value_b = b.get(name)
        if value_a != value_b:
            changed[name] = {"a": value_a, "b": value_b}
    return changed


def outside_profile(
    changed: dict, allowed: tuple[str, ...] = (PROFILE_SETTING,)
) -> list[str]:
    """Return the sorted changed names that are not in `allowed`."""
    return sorted(name for name in changed if name not in allowed)


def pair_check(changed: dict) -> tuple[bool, str]:
    """Check that a compared fingerprint pair is genuinely different.

    The check fails when the change set is empty (the two fingerprints
    are identical) or when the profile setting is unchanged. It passes
    only when the profile setting itself changed. The return value is
    `(True, "")` on success and `(False, reason)` on failure.
    """
    if not changed:
        return False, "the change set is empty: the fingerprints are identical"
    if PROFILE_SETTING not in changed:
        return False, f"{PROFILE_SETTING} did not change"
    return True, ""
