# Sweep tables

These tables are the record of the assertion-message sweep: every change is named here, and an operator-side applier (kept outside this repository) performs each one over the syntax tree.

Entry fields:

- `file`, `test`: the repository-relative file and the test function the change is scoped to.
- `old`, `new`: an exact-text splice applied only when `old` appears exactly once inside that test.
- `needle` with `last`: instead of `old`/`new`, find the assert macro whose arguments contain `needle` and strip the interpolations from its message; an empty result removes the message, and `last` selects the highest remaining match so repeated runs are stable.
- `alert`: the code-scanning alert number this entry clears, or 0 where the line carried no distinct alert.

The applier refuses to write a file whose result does not parse.
