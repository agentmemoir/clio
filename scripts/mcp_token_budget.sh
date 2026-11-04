#!/usr/bin/env bash
# Print every MCP tool-schema token-budget row for the shipped tool surface.
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
    "mcp-token-budget: no executable clio binary found (looked at CLIO_BIN, target/release/clio, target/debug/clio)." \
    "Build it with 'cargo build --locked --bin clio' or set CLIO_BIN." >&2
  exit 1
fi

python="${PYTHON:-python3}"

"$bin" mcp schema-export | "$python" "$root/benchmarks/mcp_token_budget/measure.py"
