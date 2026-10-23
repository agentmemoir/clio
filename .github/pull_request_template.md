## Summary

<!-- One or two sentences: what changes and why. -->

## Related issue

<!-- Link the issue this closes or relates to, e.g. Closes #123. -->

## Changes

<!-- What changed. List user-visible behavior first, then internals. -->

## Test plan

<!-- Commands run and results. CI runs fmt, clippy, tests, coverage, and MCP conformance. -->

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --all-features --locked`
- [ ] `cargo test --workspace --locked`
- [ ] Coverage gate passes (80% lines for every changed Rust file)

## Checklist

- [ ] The [CLA](CLA.md) comment is signed (external contributors, once), and every commit carries a `Signed-off-by` trailer that matches its author (`git commit -s`).
- [ ] No private file contents or secrets pasted into public files or CI output.
- [ ] New behavior has tests; updated docs if user-visible.
