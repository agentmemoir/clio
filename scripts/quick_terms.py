"""One selected slice of tests for Quick, as a nextest filter term.

A term names a package and narrows it: by test module, by integration target, or
by the package's own lib and bin unit tests. It renders to a nextest filterset
for the run, and it tests a JUnit case against the same slice, so the estimate
and the run read one definition. A term built here is never wider than the
package it names.
"""

from __future__ import annotations

import re
from typing import NamedTuple


class Term(NamedTuple):
    package: str
    kinds: tuple[str, ...] = ()
    binary: str | None = None
    modules: tuple[str, ...] = ()
    reason: str = ""

    @property
    def is_wide(self) -> bool:
        """True when the term names the whole package."""
        return not (self.kinds or self.binary or self.modules)

    def patterns(self) -> list[str]:
        """The anchored test-name regexes, one per owned module."""
        return [f"^{re.escape(module)}(::|$)" for module in self.modules if module]

    def expression(self) -> str:
        parts = [f"package(={self.package})"]
        if self.kinds:
            parts.append("(" + " | ".join(f"kind(={kind})" for kind in self.kinds) + ")")
        if self.binary:
            parts.append(f"binary(={self.binary})")
        found = self.patterns()
        if found:
            tests = " | ".join(f"test(/{pattern}/)" for pattern in found)
            parts.append(f"({tests})" if len(found) > 1 else tests)
        return " & ".join(parts)

    def matches(self, binary_id: str, name: str) -> bool:
        """True when nextest would select the test `name` of `binary_id`.

        nextest's binary id is the package for its lib, `package::bin/name` for a
        bin, and `package::name` for an integration target.
        """
        if binary_id != self.package and not binary_id.startswith(self.package + "::"):
            return False
        if self.kinds and not any(self._is_kind(binary_id, kind) for kind in self.kinds):
            return False
        if self.binary and binary_id != f"{self.package}::{self.binary}":
            return False
        found = self.patterns()
        return not found or any(re.search(pattern, name) for pattern in found)

    def _is_kind(self, binary_id: str, kind: str) -> bool:
        if kind == "lib":
            return binary_id == self.package
        return binary_id.startswith(f"{self.package}::{kind}/")


def union_expression(terms: list[Term]) -> str:
    """One filterset selecting any of `terms`; empty when there are none."""
    expressions = [term.expression() for term in terms]
    if len(expressions) == 1:
        return expressions[0]
    return " | ".join(f"({expression})" for expression in expressions)


def failed_expression(failed: list[tuple[str, str]]) -> str:
    """One filterset selecting exactly the (binary id, test name) pairs given."""
    return " | ".join(
        f"(binary_id(={binary_id}) & test(={name}))" for binary_id, name in failed
    )
