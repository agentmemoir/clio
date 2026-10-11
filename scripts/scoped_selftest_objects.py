"""Self-test cases for unreadable profile buffers and the instrumented objects."""

from __future__ import annotations

import io
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path, PurePosixPath

from scoped_profiles import drop_selected_objects, prune_foreign_objects

from scoped_selftest_support import shared as globals


def _run_checked(command, env=None):
    result = subprocess.run(
        command, capture_output=True, text=True, timeout=60, env=env
    )
    if result.returncode:
        raise RuntimeError(
            f"command failed: {command!r}\n{result.stdout}\n{result.stderr}"
        )
    return result


def _llvm_tools() -> tuple[Path, Path]:
    sysroot = Path(_run_checked(["rustc", "--print", "sysroot"]).stdout.strip())
    version = _run_checked(["rustc", "-vV"]).stdout
    host = next(
        line.partition(": ")[2]
        for line in version.splitlines()
        if line.startswith("host: ")
    )
    suffix = ".exe" if os.name == "nt" else ""
    tool_dir = sysroot / "lib" / "rustlib" / host / "bin"
    return tool_dir / f"llvm-profdata{suffix}", tool_dir / f"llvm-cov{suffix}"


def _report_lines(
    llvm_cov: Path, profile: Path, objects: list[Path], source: Path
) -> dict:
    command = [
        str(llvm_cov), "export", "-summary-only", f"-instr-profile={profile}"
    ]
    command.extend(f"-object={path}" for path in objects)
    report = json.loads(_run_checked(command).stdout)
    matches = [
        entry for entry in report["data"][0]["files"]
        if Path(entry["filename"]).resolve() == source.resolve()
    ]
    if len(matches) != 1:
        raise RuntimeError(f"expected one coverage row for {source}, got {matches!r}")
    return matches[0]["summary"]["lines"]


def _stale_mapping_regression(check) -> None:
    llvm_profdata, llvm_cov = _llvm_tools()
    if not llvm_profdata.is_file() or not llvm_cov.is_file():
        raise RuntimeError("the pinned Rust toolchain must include llvm-tools-preview")

    old_source = "//! old mapping\n//! shifts\n//! current lines\n" + (
        "fn measured() {\n    let value = 1;\n"
        "    std::hint::black_box(value);\n}\nfn main() { measured(); }\n"
    )
    current_source = "fn measured() {\n    let value = 1;\n" + (
        "    std::hint::black_box(value);\n}\nfn main() { measured(); }\n"
    )
    with tempfile.TemporaryDirectory(prefix="scoped-runner-stale-map-") as scratch:
        root = Path(scratch)
        source = root / "audit.rs"
        object_dir = root / "instrumented" / "debug" / "deps"
        object_dir.mkdir(parents=True)
        old_object = object_dir / "clio-aaaaaaaa"
        current_object = object_dir / "clio-bbbbbbbb"

        def compile(metadata: str, output: Path) -> None:
            _run_checked(
                [
                    "rustc", "--edition=2021", "-Cinstrument-coverage", "-Cdebuginfo=0",
                    f"-Cmetadata={metadata}", str(source), "-o", str(output),
                ]
            )

        source.write_text(old_source, encoding="utf-8")
        compile("old", old_object)
        source.write_text(current_source, encoding="utf-8")
        compile("current", current_object)

        raw_profile = root / "run.profraw"
        profile_env = dict(os.environ, LLVM_PROFILE_FILE=str(raw_profile))
        _run_checked([str(current_object)], env=profile_env)
        profile = root / "run.profdata"
        _run_checked(
            [
                str(llvm_profdata), "merge", "-sparse", str(raw_profile), "-o",
                str(profile),
            ]
        )

        inflated = _report_lines(
            llvm_cov, profile, [old_object, current_object], source
        )
        source_lines = len(current_source.splitlines())
        check(
            "old and current source maps inflate the line count and miss the floor",
            (
                inflated["count"] > source_lines,
                inflated["covered"] * 100 < inflated["count"] * 80,
            ),
            (True, True),
        )

        deleted = drop_selected_objects(root / "instrumented", ["clio"])
        check(
            "selected stale and current objects are removed before rebuild",
            (deleted, old_object.exists(), current_object.exists()),
            (2, False, False),
        )
        compile("current", current_object)
        clean = _report_lines(llvm_cov, profile, [current_object], source)
        check(
            "the clean report stays within source lines and clears the floor",
            (
                clean["count"] <= source_lines,
                clean["covered"] * 100 >= clean["count"] * 80,
            ),
            (True, True),
        )


def run(check, raises) -> tuple:
    # --- the instrumented objects the report is allowed to see ---------------
    original_packages_for_prune = globals()["workspace_packages"]
    prune_packages = [
        {"name": "clio", "deps": set(), "dir": "crates/clio",
         "targets": ["clio", "additive_tools_test"]},
        {"name": "clio-store", "deps": set(), "dir": "crates/clio-store",
         "targets": ["clio-store", "mcp_principal_binding"]},
        {"name": "clio-ops", "deps": set(), "dir": "crates/clio-ops",
         "targets": ["clio-ops", "clio_ops_conformance"]},
    ]
    try:
        globals()["workspace_packages"] = lambda: prune_packages
        with tempfile.TemporaryDirectory(prefix="scoped-runner-objects-") as scratch:
            object_root = Path(scratch)
            (object_root / "debug" / "deps").mkdir(parents=True)
            deps = object_root / "debug" / "deps"
            kept = [
                # The selected packages' own binaries. Cargo spells the file name
                # of `clio-store`'s test binary with an UNDERSCORE, so a rule
                # matching only the dashed package name would delete a binary the
                # run needs.
                deps / "clio-2b3cf5c1458a8f42",
                deps / "clio_store-98ac1e2f3b4d5c6e",
                # A selected package's integration-test targets, which are named
                # after the TARGET and not after the package.
                deps / "mcp_principal_binding-7f6e5d4c3b2a1908",
                deps / "additive_tools_test-1011ac0a5f159ade",
            ]
            deleted = [
                deps / "clio_ops-fa1e5933542c28db",
                deps / "clio_ops_conformance-5150f1e2d3c4b5a6",
            ]
            # `clio` is the workspace root's package and `clio-store` is a
            # different package whose name starts with it. A prefix rule, or one
            # that only matched the dashed spelling, would get this pair wrong.
            for name in kept + deleted:
                name.write_text("x")
                os.chmod(name, 0o755)
            decoy_library = deps / "clio_store-1a2b3c4d5e6f7081.rlib"
            decoy_library.write_text("x")
            decoy_depinfo = deps / "clio_ops-1a2b3c4d5e6f7081.d"
            decoy_depinfo.write_text("x")
            prune_log = io.StringIO()
            original_stdout_for_prune = sys.stdout
            try:
                sys.stdout = prune_log
                deleted_count = prune_foreign_objects(
                    object_root, ["clio", "clio-store"]
                )
            finally:
                sys.stdout = original_stdout_for_prune
            check(
                "every selected package's binary and target binary survives",
                [path.exists() for path in kept],
                [True, True, True, True],
            )
            check(
                "an unselected package's binary is deleted",
                [path.exists() for path in deleted],
                [False, False],
            )
            check("the number deleted is printed", "deleted 2" in prune_log.getvalue(), True)
            check("the number deleted is returned", deleted_count, 2)
            # These two are named exactly like the objects above and are not
            # objects to the merge, so deleting them would cost a rebuild and fix
            # nothing.
            check("a library archive is not an object and survives", decoy_library.exists(), True)
            check("a dependency-info file survives", decoy_depinfo.exists(), True)

        # The pair that a prefix rule gets wrong, on its own: selecting only
        # `clio-store` keeps `clio_store-<hash>` and deletes `clio-<hash>`.
        with tempfile.TemporaryDirectory(prefix="scoped-runner-prefix-") as scratch:
            prefix_root = Path(scratch) / "llvm-cov-target"
            prefix_deps = prefix_root / "debug" / "deps"
            prefix_deps.mkdir(parents=True)
            store_binary = prefix_deps / "clio_store-98ac1e2f3b4d5c6e"
            root_binary = prefix_deps / "clio-2b3cf5c1458a8f42"
            for name in (store_binary, root_binary):
                name.write_text("x")
                os.chmod(name, 0o755)
            original_stdout_for_prefix = sys.stdout
            try:
                sys.stdout = io.StringIO()
                prune_foreign_objects(prefix_root, ["clio-store"])
            finally:
                sys.stdout = original_stdout_for_prefix
            check("selecting a package keeps its own underscored binary", store_binary.exists(), True)
            check(
                "one package's name being a prefix of another's does not keep it",
                root_binary.exists(),
                False,
            )

        # A run that deletes nothing still has to say so, because the number is
        # the evidence that the pruning happened at all.
        with tempfile.TemporaryDirectory(prefix="scoped-runner-all-") as scratch:
            all_root = Path(scratch)
            only = all_root / "clio-2b3cf5c1458a8f42"
            only.write_text("x")
            os.chmod(only, 0o755)
            all_log = io.StringIO()
            original_stdout_for_all = sys.stdout
            try:
                sys.stdout = all_log
                none_deleted = prune_foreign_objects(
                    all_root, ["clio", "clio-store", "clio-ops"]
                )
            finally:
                sys.stdout = original_stdout_for_all
            check(
                "selecting every package deletes nothing",
                (none_deleted, only.exists(), "deleted 0" in all_log.getvalue()),
                (0, True, True),
            )

        # Old and current selected-package binaries can both match; remove both before rebuilding.
        with tempfile.TemporaryDirectory(prefix="scoped-runner-selected-") as scratch:
            selected_root = Path(scratch)
            selected_deps = selected_root / "debug" / "deps"
            selected_deps.mkdir(parents=True)
            stale = selected_deps / "clio-2b3cf5c1458a8f42"
            current = selected_deps / "clio-1011ac0a5f159ade"
            target = selected_deps / "additive_tools_test-98ac1e2f3b4d5c6e"
            foreign = selected_deps / "clio_ops-fa1e5933542c28db"
            for path in (stale, current, target, foreign):
                path.write_text("x")
                os.chmod(path, 0o755)
            selected_log = io.StringIO()
            original_stdout_for_selected = sys.stdout
            try:
                sys.stdout = selected_log
                selected_count = drop_selected_objects(selected_root, ["clio"])
            finally:
                sys.stdout = original_stdout_for_selected
            check(
                "old and current binaries of a selected package are removed",
                [stale.exists(), current.exists(), target.exists()],
                [False, False, False],
            )
            check("a foreign package's binary remains for the later prune", foreign.exists(), True)
            check("the selected object count is printed", "deleted 3" in selected_log.getvalue(), True)
            check("the number of selected objects deleted is returned", selected_count, 3)

        # Reproduce the line-map union that made a doc-comment-only edit fail.
        _stale_mapping_regression(check)

        # An empty selection is a mass deletion, not an empty deletion. With no
        # names the keep pattern matches only the empty stem, so every object it
        # can see goes. The caller returns early on an empty selection, so this
        # path is unreachable from the command line today; the refusal lives in the
        # function so it does not depend on a caller two frames away.
        with tempfile.TemporaryDirectory(prefix="scoped-runner-empty-prune-") as scratch:
            empty_root = Path(scratch)
            doomed = empty_root / "clio-store-1a2b3c4d"
            doomed.write_bytes(b"\x7fELF fake executable")
            check(
                "an empty selection is refused before anything is deleted",
                raises(prune_foreign_objects, empty_root, []),
                True,
            )
            check("and the refused run deleted nothing", doomed.exists(), True)
            check(
                "an empty pre-run selection is also refused",
                raises(drop_selected_objects, empty_root, []),
                True,
            )
            check("the pre-run refusal leaves the object in place", doomed.exists(), True)
    finally:
        globals()["workspace_packages"] = original_packages_for_prune
    return name, prune_packages, scratch
