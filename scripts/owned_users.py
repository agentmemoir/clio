"""The tests that use a changed file which owns no test module of its own.

A fixture, a test helper, a shared `tests/common/mod.rs`, or a production module with no
tests owns no test module, and Quick ran its whole package for it. This module finds the
tests that use the file instead, from the declarations `owned_tests` already reads:

- An integration target uses each file its module tree reaches. Cargo builds
  `tests/common/mod.rs` into every target that declares `mod common;`, and into no target
  of its own.
- A module is used by the module that declares it, and by each file that names it outside
  comments. A user's own test modules are selected; a user that owns none is followed in
  turn. An integration target that names it is selected too.

The answer is empty, with the reason, when no test names the file, or when a crate root
names it: the root re-exports it, so any file may use it under another name. The caller
then runs the whole package.
"""

from __future__ import annotations

import functools
import re
from pathlib import Path
from typing import NamedTuple

import owned_tests as owned
from integration_targets import target_entries
from owned_scanner import read_text, strip_comments, test_modules_in

NO_USER = "no test names it"
ROOT_USER = "the crate root names it"
_PATH_ATTR = re.compile(r'#\s*\[\s*path\s*=\s*"[^"]*"\s*\]')


class Users(NamedTuple):
    modules: tuple[str, ...] = ()
    binaries: tuple[str, ...] = ()
    reason: str = ""


_raw = functools.lru_cache(maxsize=None)(read_text)


@functools.lru_cache(maxsize=None)
def _code(path: Path) -> str:
    """The source without comments and `#[path]` attributes, which name files, not uses."""
    return _PATH_ATTR.sub("", strip_comments(_raw(path)))


def names(path: Path, name: str) -> bool:
    """True when the file names module `name` anywhere but in its own `mod name;`."""
    if name not in _raw(path):
        return False
    code = re.sub(rf"\bmod\s+{name}\s*;", "", _code(path))
    return re.search(rf"\b{name}\b", code) is not None


def users_of(rel: str) -> Users:
    crate_dir = owned.crate_of(rel)
    changed = (owned.ROOT / rel).resolve()
    trees = {name: owned.walk_from((entry,)) for name, entry in target_entries(crate_dir).items()}
    binaries = {name for name, tree in trees.items() if any(p.resolve() == changed for p in tree)}
    module_of = {path.resolve(): module for path, module in owned.walk_crate(crate_dir).items()}
    modules: set[str] = set()
    queue = [changed] if changed in module_of else []
    seen = set(queue)
    while queue:
        path = queue.pop()
        module = module_of[path]
        name = module.rpartition("::")[2]
        binaries |= {target for target, tree in trees.items() if any(names(p, name) for p in tree)}
        parent = max((p for p, m in module_of.items() if module.startswith(f"{m}::") or not m), key=lambda p: len(module_of[p]))
        for user in sorted(p for p in module_of if p not in seen and (p == parent or names(p, name))):
            if not module_of[user]:
                if names(user, name):
                    return Users(reason=ROOT_USER)
                continue
            seen.add(user)
            found = test_modules_in(_raw(user), module_of[user], user)
            modules.update(found)
            if not found:
                queue.append(user)
    if not modules and not binaries:
        return Users(reason=NO_USER)
    return Users(tuple(sorted(modules)), tuple(sorted(binaries)))
