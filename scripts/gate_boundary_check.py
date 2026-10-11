#!/usr/bin/env python3
"""Checks that only the gated admission path creates long-term items.

The repository primitives (`Store::create_memory_item`,
`Store::commit_distilled_unit`, and the `BatchWrite::CreateItem` arm) stay public
so the admission wrapper can call them, which leaves a bypass one direct call
away. This check is the tripwire: no `.rs` file under `crates/` may name those
primitives outside the allowlists below, and the legacy `ItemEnvelope` CRUD stays
removed.

It is a script, not a `#[test]` in `clio-store`, because its subject is the whole
workspace while a scoped run selects packages from the diff. `clio-store` is a
dependency of most crates, so a change to another crate usually does not select
it, and a test there never executes for exactly the changes least likely to be
reviewed closely. A check whose subject is broader than its own crate has to run
outside the crate selection, which is the `if: always()` block in the workflow
and `make scripts-check` locally. It reads source and runs no build.

The allowlists are the only signal a writer gets. `allowed_caller` exempts
`*_tests.rs`, `*_test_support.rs`, `*_fixtures.rs`, `*/tests/*`, and a named list
of production files that persist only inside an admitted write closure. A helper
that writes must be listed in the same change that creates it.

Usage:
  python3 scripts/gate_boundary_check.py
  python3 scripts/gate_boundary_check.py --self-test

Exit codes:
  0  no offender, every scanned file was read
  1  at least one offender, each naming the file and line
  2  usage error
  3  a file or directory this check had to read could not be read
"""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path

EXIT_OK = 0
EXIT_FINDINGS = 1
EXIT_USAGE = 2
EXIT_UNREADABLE = 3

USAGE = "usage: gate_boundary_check.py | --self-test"

CRATES = "crates"

NEEDLE_CREATE = "create_memory_item("
NEEDLE_DISTILL = "commit_distilled_unit("
NEEDLE_BATCH = "BatchWrite::CreateItem"
LEGACY_NEEDLES = ("ItemEnvelope", ".put_item(", ".get_item(")


def _test_only(path: str) -> bool:
    """Test modules and the shared test helpers, by the naming convention.

    `foo_test_support.rs` and `foo_fixtures.rs` are declared `#[cfg(test)]` and
    never built into production. `foo_seam.rs` is deliberately NOT exempt: a seam
    can be compiled into production, so the name alone does not prove the file is
    test-only.
    """
    return (
        path.endswith("_tests.rs")
        or path.endswith("_test_support.rs")
        or path.endswith("_fixtures.rs")
        or "/tests/" in path
    )


#: Files allowed to name `create_memory_item(` directly.
CALLER_NAMED = frozenset(
    {
        "crates/clio-store/src/store.rs",
        "crates/clio-store/src/sqlite.rs",
        "crates/clio-store/src/sqlite_store.rs",
        "crates/clio-store/src/postgres.rs",
        "crates/clio-store/src/postgres_store.rs",
        "crates/clio-store/src/async_store.rs",
        "crates/clio-admission/src/gated.rs",
        # The MCP `store` tool persists through `clio_write::gated_store_verified`
        # (which wraps `gated_create`) and only calls the primitive inside that
        # wrapper's write closure.
        "crates/clio-mcp/src/store_write.rs",
        # The library raw-ingest API runs `clio_write::parallel_ingest`, which
        # persists only inside `gated_store_verified`'s write closure after span
        # verify and admission pass.
        "crates/clio-write/src/raw_ingest.rs",
        # Sync apply persists ALREADY-ADMITTED peer creates: the convergence rule
        # forbids re-running local admission on trusted remote creates.
        "crates/clio-sync/src/apply.rs",
        # The MCP `import_provider` adapter admits through
        # `clio_admission::gated_create_raw`.
        "crates/clio-mcp/src/provider_adapter.rs",
    }
)

#: Files allowed to name `commit_distilled_unit(`. Hub distillation is an admitted
#: second write surface: it runs `clio_admission::gated_create` and persists
#: through this primitive inside that wrapper's write closure.
DISTILL_NAMED = frozenset(
    {
        "crates/clio-store/src/store.rs",
        "crates/clio-store/src/sqlite_store.rs",
        "crates/clio-store/src/postgres_store.rs",
        "crates/clio-write/src/hub_distill.rs",
    }
)

#: Files allowed to stage `BatchWrite::CreateItem`. The same already-admitted and
#: first-gate rules apply.
BATCH_NAMED = frozenset(
    {
        "crates/clio-store/src/batch_sqlite.rs",
        "crates/clio-store/src/batch_postgres.rs",
        # Compliance import restores an exported bundle verbatim: the exported
        # rows were already admitted.
        "crates/clio-compliance/src/import_apply.rs",
        # The MCP batch tool stages `BatchWrite` ops only after `batch_preflight`
        # scores every op through `gated_store_verified`.
        "crates/clio-mcp/src/batch_preflight.rs",
        "crates/clio-mcp/src/batch_tools.rs",
    }
)


def allowed_caller(path: str) -> bool:
    return _test_only(path) or path in CALLER_NAMED


def allowed_distill_committer(path: str) -> bool:
    return _test_only(path) or path in DISTILL_NAMED


def allowed_batch_caller(path: str) -> bool:
    return _test_only(path) or path in BATCH_NAMED


def allowed_legacy(path: str) -> bool:
    """No `.rs` file is exempt: the legacy needles live in this Python check."""
    return False


class Scan:
    def __init__(self, needles: tuple[str, ...], allowed, guidance: str) -> None:
        self.needles = needles
        self.allowed = allowed
        self.guidance = guidance


SCANS = (
    Scan(
        (NEEDLE_CREATE,),
        allowed_caller,
        "route new long-term writes through clio-admission gated_create",
    ),
    Scan(
        (NEEDLE_DISTILL,),
        allowed_distill_committer,
        "hub distillation persists through Store::commit_distilled_unit inside "
        "clio-admission gated_create's write closure; route new writes through an "
        "admitted surface",
    ),
    Scan(
        (NEEDLE_BATCH,),
        allowed_batch_caller,
        "score every op through clio-admission gating before apply_batch",
    ),
    Scan(
        LEGACY_NEEDLES,
        allowed_legacy,
        "opaque at-rest rows arrive only through BatchWrite::PutOpaqueItem, and "
        "typed items go through create_memory_item",
    ),
)


class Finding:
    def __init__(self, path: str, line: int, needle: str, guidance: str) -> None:
        self.path = path
        self.line = line
        self.needle = needle
        self.guidance = guidance

    def __str__(self) -> str:
        return f"{self.path}:{self.line}: {self.needle} ({self.guidance})"


class Surface:
    def __init__(self) -> None:
        self.findings: list[Finding] = []
        self.unreadable: list[str] = []
        self.scanned = 0

    def exit_code(self) -> int:
        if self.unreadable:
            return EXIT_UNREADABLE
        return EXIT_FINDINGS if self.findings else EXIT_OK


def rs_files(root: Path) -> list[Path]:
    base = root / CRATES
    if not base.is_dir():
        raise FileNotFoundError(f"{CRATES}: no source directory to scan")
    return sorted(path for path in base.rglob("*.rs") if path.is_file())


def scan(root: Path) -> Surface:
    surface = Surface()
    try:
        files = rs_files(root)
    except FileNotFoundError as error:
        surface.unreadable.append(str(error))
        return surface
    except OSError as error:
        surface.unreadable.append(f"{CRATES}: {error.strerror or error}")
        return surface
    if not files:
        surface.unreadable.append(f"{CRATES}: no Rust source file was found to scan")
        return surface

    contents: dict[str, str] = {}
    for file in files:
        relative = file.relative_to(root).as_posix()
        try:
            contents[relative] = file.read_text(encoding="utf-8", errors="replace")
        except OSError as error:
            surface.unreadable.append(f"{relative}: {error.strerror or error}")
    surface.scanned = len(contents)

    for spec in SCANS:
        for relative, content in contents.items():
            if spec.allowed(relative):
                continue
            for number, line in enumerate(content.splitlines(), start=1):
                if line.lstrip().startswith("//"):
                    continue
                for needle in spec.needles:
                    if needle in line:
                        surface.findings.append(
                            Finding(relative, number, needle, spec.guidance)
                        )
    return surface


def report(root: Path) -> int:
    surface = scan(root)
    for path in surface.unreadable:
        print(f"gate-boundary: COULD NOT READ {path}", file=sys.stderr)
    for finding in surface.findings:
        print(f"gate-boundary: FINDING {finding}")
    code = surface.exit_code()
    if code == EXIT_UNREADABLE:
        print(
            f"gate-boundary: {len(surface.unreadable)} path(s) this check had to read "
            "could not be read, so the rest of this run does not cover them",
            file=sys.stderr,
        )
        return code
    if code == EXIT_FINDINGS:
        print(f"gate-boundary: {len(surface.findings)} finding(s)")
        return code
    print(f"gate-boundary: no offender in {surface.scanned} source file(s)")
    return code


def _write(root: Path, relative: str, text: str) -> None:
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def _fixture(base: Path, label: str, files: dict[str, str]) -> Path:
    root = base / label
    for relative, text in files.items():
        _write(root, relative, text)
    return root


def _cases(base: Path) -> list[tuple[str, int, Path]]:
    allowed = {
        "crates/clio-store/src/store.rs": f"fn w() {{ {NEEDLE_CREATE} }}\n",
        "crates/clio-admission/src/gated.rs": f"fn w() {{ {NEEDLE_CREATE} }}\n",
        "crates/clio-mcp/src/helper_tests.rs": f"fn t() {{ {NEEDLE_CREATE} }}\n",
        "crates/clio-mcp/src/helper_test_support.rs": f"fn t() {{ {NEEDLE_CREATE} }}\n",
        "crates/clio-mcp/src/helper_fixtures.rs": f"fn t() {{ {NEEDLE_CREATE} }}\n",
        "crates/clio-mcp/tests/integration.rs": f"fn t() {{ {NEEDLE_CREATE} }}\n",
        "crates/clio-write/src/hub_distill.rs": f"fn w() {{ {NEEDLE_DISTILL} }}\n",
        "crates/clio-mcp/src/batch_tools.rs": f"fn w() {{ {NEEDLE_BATCH} }}\n",
        "crates/clio-mcp/src/comment.rs": f"// {NEEDLE_CREATE}\nfn nothing() {{}}\n",
    }
    plain = "crates/clio-mcp/src/leaf_helper.rs"
    return [
        ("every allowlist shape is exempt", EXIT_OK, _fixture(base, "clean", allowed)),
        (
            "a direct create caller in a leaf crate is a finding",
            EXIT_FINDINGS,
            _fixture(base, "create", {plain: f"fn w() {{ {NEEDLE_CREATE} }}\n"}),
        ),
        (
            "a direct distill caller is a finding",
            EXIT_FINDINGS,
            _fixture(base, "distill", {plain: f"fn w() {{ {NEEDLE_DISTILL} }}\n"}),
        ),
        (
            "a direct batch caller is a finding",
            EXIT_FINDINGS,
            _fixture(base, "batch", {plain: f"fn w() {{ {NEEDLE_BATCH} }}\n"}),
        ),
        (
            "the legacy type is a finding",
            EXIT_FINDINGS,
            _fixture(base, "legacy-type", {plain: "fn w() -> ItemEnvelope {}\n"}),
        ),
        (
            "a legacy method is a finding",
            EXIT_FINDINGS,
            _fixture(base, "legacy-method", {plain: "fn w() { s.put_item(); }\n"}),
        ),
        (
            "a missing crates directory is not a pass",
            EXIT_UNREADABLE,
            _fixture(base, "no-crates", {"README.md": "no crates here\n"}),
        ),
        (
            "an empty crates directory is not a pass",
            EXIT_UNREADABLE,
            _fixture(base, "empty", {"crates/.keep": ""}),
        ),
    ]


def _self_test() -> int:
    failures = 0
    total = 0
    with tempfile.TemporaryDirectory(prefix="gate-boundary-") as scratch:
        base = Path(scratch)
        for name, want, root in _cases(base):
            total += 1
            got = scan(root).exit_code()
            ok = got == want
            print(f"self-test {'ok' if ok else 'FAIL'}: {name} (exit {got}, want {want})")
            failures += 0 if ok else 1

        # A finding that names the file but not the line leaves a reader with a
        # large file and no idea which call is the offender.
        total += 1
        planted = _fixture(
            base,
            "names-line",
            {"crates/clio-mcp/src/leaf_helper.rs": "fn a() {}\n\nfn w() {\n    s.create_memory_item();\n}\n"},
        )
        findings = scan(planted).findings
        want_line = 4
        ok = len(findings) == 1 and findings[0].line == want_line
        print(
            f"self-test {'ok' if ok else 'FAIL'}: a finding names the line "
            f"({[f.line for f in findings]}, want [{want_line}])"
        )
        failures += 0 if ok else 1
    if failures:
        print(f"self-test: {failures} of {total} failed")
        return EXIT_FINDINGS
    print(f"self-test: all passed ({total} cases)")
    return EXIT_OK


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return _self_test()
    if "-h" in argv or "--help" in argv:
        print(__doc__.strip())
        return EXIT_OK
    if argv:
        print(USAGE, file=sys.stderr)
        return EXIT_USAGE
    return report(Path(__file__).resolve().parent.parent)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
