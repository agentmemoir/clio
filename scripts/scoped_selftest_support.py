"""The shared namespace the scoped runner's self-test chunks patch.

The self-test used to be one function inside `scoped_runner.py`, and it replaced module-level
names with `globals()["name"] = fake` and put them back afterwards. Every function then saw the
fake, because they all shared one namespace. The chunks now live in modules of their own, so each
chunk file takes `shared` as `globals` and the same lines keep their meaning: a read returns the
name from the first `scoped_*` module that has it, and a write sets it in every `scoped_*` module
that has it, so the code under test and the test code both see the fake.
"""

from __future__ import annotations

import sys
from types import ModuleType


def _modules() -> list[ModuleType]:
    return [m for name, m in sorted(sys.modules.items()) if m is not None and name.startswith("scoped_")]


class _Namespace:
    def __getitem__(self, name: str):
        for module in _modules():
            if hasattr(module, name):
                return getattr(module, name)
        if name in sys.modules:
            return sys.modules[name]
        raise KeyError(name)

    def __setitem__(self, name: str, value) -> None:
        found = [module for module in _modules() if hasattr(module, name)]
        if not found:
            raise KeyError(f"no scoped_* module has {name!r} to replace")
        for module in found:
            setattr(module, name, value)


def shared() -> _Namespace:
    return _Namespace()
