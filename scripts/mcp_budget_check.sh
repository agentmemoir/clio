#!/usr/bin/env bash
# Check the shipped MCP tool-surface pack against the frozen budget baseline.
# This is the one local entry point, and the budget job runs this same script.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd -- "$script_dir/.." && pwd)"

bin="${CLIO_BIN:-}"
if [[ -z "$bin" ]]; then
  for candidate in "$root/target/release/clio" "$root/target/debug/clio"; do
    if [[ -x "$candidate" ]]; then
      bin="$candidate"
      break
    fi
  done
fi
if [[ -z "$bin" || ! -x "$bin" ]]; then
  printf '%s\n' \
    "mcp-budget-check: no executable clio binary found (looked at CLIO_BIN, target/release/clio, target/debug/clio)." \
    "Build it with 'cargo build --locked --bin clio' or set CLIO_BIN." >&2
  exit 2
fi

tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

# Export first and check its status. The temp file is pack data, never executed.
if ! "$bin" mcp schema-export >"$tmp"; then
  printf '%s\n' "mcp-budget-check: clio mcp schema-export failed" >&2
  exit 2
fi

args=(--pack "$tmp")
if [[ -n "${CLIO_SKILL_ENTRY_POINT:-}" ]]; then
  args+=(--skill "$CLIO_SKILL_ENTRY_POINT")
fi

# Run the checker in this shell so the temp pack file is always removed.
status=0
python3 "$root/benchmarks/mcp_token_budget/budget_check.py" "${args[@]}" || status=$?

rm -f "$tmp"
trap - EXIT
exit "$status"
