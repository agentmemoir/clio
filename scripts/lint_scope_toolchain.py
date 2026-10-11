"""Refusing a clippy run that picks its own toolchain instead of the pinned one."""

from __future__ import annotations

import re

from lint_scope_text import ENV_ASSIGNMENT, SHIM_EXECUTABLES, _bare_shim, _unwrap_variable

#: The environment variables that decide which toolchain a run uses rather than
#: which packages it selects, so a clippy line carrying one is refused. All four
#: are here because this Makefile exports or sets them, and an export reaches a
#: recipe whether or not the line itself names it.
TOOLCHAIN_OVERRIDES = frozenset({"RUSTUP_TOOLCHAIN", "RUSTC", "RUSTDOC", "CARGO"})


def _toolchain_problem(
    executable: str,
    line: str,
    selections: dict[str, list[list[str]]],
) -> str | None:
    """Why this clippy run does not read the pinned toolchain, or None.

    A toolchain binary invoked by path does not read `rust-toolchain.toml`, so a
    run that reaches cargo through one lints with whichever compiler that
    toolchain directory happens to hold. Once the pin moves, that is a different
    compiler from the one the runner uses, and the local gate reports green on a
    tree the runner rejects. Only a bare name reaches cargo through the shim, and
    the shim is what reads the pin.

    A `$(NAME)` or `${NAME}` reference is judged on the definitions rather than
    on its own text, which is what makes `CARGO ?= cargo` pass: the reference
    expands to a bare `cargo`, so the run does read the pin. It is refused when
    any definition of that name is a path, because make takes the last assignment
    before use and one path is enough, and when the file never defines the name,
    because then nothing here can be shown to be a bare `cargo` -- the `$(CARGO)`
    case in a workflow, where no Makefile was read at all.

    A definition naming another variable is refused rather than followed. That is
    this rule's conservative direction: one level of indirection has no
    definition here to be checked against, and this Makefile needs none.
    """
    raw = executable.strip("\"'`")
    if raw.lower() in SHIM_EXECUTABLES:
        return None
    name, _ = _unwrap_variable(executable)
    if name is None:
        because = f"a clippy run through `{raw}`, which is not a bare `cargo`. "
    elif name not in selections:
        because = (
            f"a clippy run through `{raw}`, which this file never defines. What "
            "it expands to cannot be shown to be a bare `cargo`, so a definition "
            "is required before the reference is trusted. "
        )
    elif not all(_bare_shim(value) for value in selections[name]):
        because = (
            f"a clippy run through `{raw}`, and not every definition of `{name}` "
            "in this file is a bare `cargo`. Make takes the last assignment "
            "before use, so one definition carrying a toolchain path is enough. "
        )
    else:
        return None
    return (
        f"{because}A toolchain binary invoked by path does not read "
        "`rust-toolchain.toml`, so the run lints with the compiler this host "
        f"carries rather than the one the repository pins: `{line}`"
    )


def _toolchain_pin_suffix(after: list[str], line: str) -> str | None:
    """Why the tokens after the cargo executable name the toolchain, or None.

    `cargo +nightly clippy` is rustup's own first-documented override, and
    `rustup help run` states it is the same thing as `rustup run --install
    nightly cargo`. It sits *after* the executable rather than before it, so
    `_toolchain_override` cannot see it: there is nothing in front of the cargo
    token to read. Accepted with exit 0, it lints with whichever compiler the
    name selects and never reads the toolchain file, which is the defect this
    rule exists for.

    Only the first token after the executable is read, because that is where
    rustup expects the override and a `+name` anywhere else is an argument to
    clippy rather than a toolchain. It must be a plain name: `+` alone is not a
    toolchain, and refusing it would refuse a spelling that names nothing.
    """
    if not after or not after[0].startswith("+"):
        return None
    named = after[0][1:]
    if not named or not re.fullmatch(r"[A-Za-z0-9._-]+", named):
        return None
    return (
        f"a clippy run under `cargo +{named}`, which names the toolchain on the "
        "command line. Rustup resolves a named toolchain ahead of the toolchain "
        "file, so the pin is not read and the run lints with the compiler that "
        f"name selects rather than the one the repository pins: `{line}`"
    )


def _toolchain_override(prefix: list[str], line: str) -> str | None:
    """Why the tokens before the cargo executable name the toolchain, or None.

    Only the executable itself was read before, so a line that reached cargo
    through the shim and then chose a toolchain around it passed. Two such lines
    were accepted with exit 0 while linting with a compiler the repository does
    not pin: `rustup run stable cargo clippy` and `RUSTUP_TOOLCHAIN=stable cargo
    clippy`.

    `rustup run` is refused only when it actually names a toolchain. With no name
    between `run` and cargo there is nothing to override, and on rustup 1.29.1
    `<TOOLCHAIN>` is a required argument, so that spelling is not a lint
    invocation at all. Refusing every line containing `rustup` would refuse the
    one spelling that is safe along with the one that is not.
    """
    for index, token in enumerate(prefix):
        if token != "rustup":
            continue
        rest = prefix[index + 1 :]
        if rest and rest[0].startswith("+"):
            spelling = rest[0]
        elif len(rest) > 1 and rest[0] == "run":
            spelling = f"run {rest[1]}"
        else:
            continue
        return (
            f"a clippy run under `rustup {spelling}`, which names the toolchain on "
            "the command line. Rustup resolves a named toolchain ahead of the "
            "toolchain file, so the pin is not read and the run lints with the "
            "compiler that name selects rather than the one the repository pins: "
            f"`{line}`"
        )
    for index, token in enumerate(prefix):
        match = ENV_ASSIGNMENT.match(token)
        if match is not None:
            name, value = match.group("name"), match.group("value")
        elif (
            token in TOOLCHAIN_OVERRIDES
            and len(prefix) > index + 1
            and prefix[index + 1] == "="
        ):
            # `NAME = value` splits into three tokens, so the value is the one
            # after the `=`.
            name, value = token, prefix[index + 2] if len(prefix) > index + 2 else ""
        else:
            continue
        if name not in TOOLCHAIN_OVERRIDES:
            continue
        if name == "RUSTUP_TOOLCHAIN":
            lead = (
                f"`{name}={value}` is an override rustup resolves ahead of the "
                "toolchain file, so the pin is never read"
            )
        elif name == "CARGO":
            lead = (
                f"`{name}={value}` names the cargo a subcommand is handed, which "
                "is the same toolchain choice as reaching cargo by path"
            )
        else:
            lead = (
                f"`{name}={value}` is the tool cargo was told to run instead of "
                "the one it would pick, so the toolchain file does not reach it"
            )
        return (
            f"a clippy run behind {lead}. The run then lints with the compiler "
            f"this line names rather than the one the repository pins: `{line}`"
        )
    return None
