"""The coverage-surface self-test cases for rule 4, the hygiene steps around a coverage run."""

from __future__ import annotations

from pathlib import Path

from coverage_surface_core import EXIT_FINDINGS, EXIT_OK, PACKAGE_RUNNER, RUNNER
from coverage_surface_fixtures import (
    CLEAN_PACKAGE_RUNNER,
    COMMENTED_AND_CALLED_RUNNER,
    COMMENTED_OBJECT_DELETE_RUNNER,
    HYGIENE_AFTER_REPORT_RUNNER,
    LATE_SELECTED_OBJECT_DROP_RUNNER,
    NO_BUFFER_DROP_RUNNER,
    NO_OBJECT_DELETE_RUNNER,
    NO_SELECTED_OBJECT_DROP_RUNNER,
    _fixture,
)


def hygiene_cases(base: Path) -> list[tuple[str, int, Path, list[str]]]:
    """(name, expected exit, fixture root, rules it must prove) for rule 4."""
    return [
        # Rule 4, the hygiene steps. Each of these leaves rule 1 satisfied, because
        # the file still pins a directory of its own for its other mode, which is
        # the whole reason this rule cannot be folded into rule 1.
        (
            "a run that never deletes the objects it did not build is a finding",
            EXIT_FINDINGS,
            _fixture(base, "no-object-delete", {RUNNER: NO_OBJECT_DELETE_RUNNER}),
            ["hygiene"],
        ),
        (
            "a run that keeps stale selected objects for the report is a finding",
            EXIT_FINDINGS,
            _fixture(
                base,
                "no-selected-object-drop",
                {RUNNER: NO_SELECTED_OBJECT_DROP_RUNNER},
            ),
            ["hygiene"],
        ),
        (
            "a run that drops stale selected objects after the report is a finding",
            EXIT_FINDINGS,
            _fixture(
                base,
                "late-selected-object-drop",
                {RUNNER: LATE_SELECTED_OBJECT_DROP_RUNNER},
            ),
            ["hygiene"],
        ),
        (
            "a one-package run that keeps old selected objects is a finding",
            EXIT_FINDINGS,
            _fixture(
                base,
                "package-no-selected-object-drop",
                {
                    PACKAGE_RUNNER: CLEAN_PACKAGE_RUNNER.replace(
                        "    drop_stale_objects(directory, [name], produced)\n", ""
                    )
                },
            ),
            ["hygiene"],
        ),
        (
            "a run that never drops an unreadable profile buffer is a finding",
            EXIT_FINDINGS,
            _fixture(base, "no-buffer-drop", {RUNNER: NO_BUFFER_DROP_RUNNER}),
            ["hygiene"],
        ),
        (
            "a one-package run that never drops an unreadable profile buffer is a finding",
            EXIT_FINDINGS,
            _fixture(base, "package-no-buffer-drop", {PACKAGE_RUNNER: CLEAN_PACKAGE_RUNNER.replace("    drop_invalid_profraws(directory)\n", "")}),
            ["hygiene"],
        ),
        (
            "a multi-line docstring does not hide the code after it from rule 4",
            EXIT_FINDINGS,
            _fixture(
                base,
                "multiline-docstring",
                {RUNNER: NO_OBJECT_DELETE_RUNNER.replace('"""A fixture gate."""', '"""A fixture gate.\n\nA second paragraph.\n"""')},
            ),
            ["hygiene"],
        ),
        (
            "a step named only in a trailing comment is a finding",
            EXIT_FINDINGS,
            _fixture(
                base, "commented-object-delete", {RUNNER: COMMENTED_OBJECT_DELETE_RUNNER}
            ),
            ["hygiene"],
        ),
        (
            "a real call carrying a trailing comment is not a finding",
            EXIT_OK,
            _fixture(base, "commented-and-called", {RUNNER: COMMENTED_AND_CALLED_RUNNER}),
            [],
        ),
        (
            "a run that writes its report before cleaning is a finding",
            EXIT_FINDINGS,
            _fixture(base, "hygiene-after", {RUNNER: HYGIENE_AFTER_REPORT_RUNNER}),
            ["hygiene", "hygiene", "hygiene"],
        ),
    ]
