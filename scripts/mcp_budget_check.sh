#!/usr/bin/env bash
# Check the shipped MCP tool-surface pack against the frozen budget baseline.
# This is the one local entry point, and the budget job runs this same script.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd -- "$script_dir/.." && pwd)"
python="${PYTHON:-python3}"

# Use CLIO_BIN when set, otherwise the newest built binary by mtime, so a fresh
# debug build is never shadowed by a stale release build. The choice is printed
# below so a run is auditable.
bin="${CLIO_BIN:-}"
if [[ -z "$bin" ]]; then
  newest=0
  for candidate in "$root/target/release/clio" "$root/target/debug/clio"; do
    if [[ -x "$candidate" ]]; then
      if mtime="$(stat -c %Y "$candidate" 2>/dev/null)"; then
        :
      else
        mtime="$(stat -f %m "$candidate")"
      fi
      if (( mtime > newest )); then
        newest="$mtime"
        bin="$candidate"
      fi
    fi
  done
fi
if [[ -z "$bin" || ! -x "$bin" ]]; then
  printf '%s\n' \
    "mcp-budget-check: no executable clio binary found (looked at CLIO_BIN, target/release/clio, target/debug/clio)." \
    "Build it with 'cargo build --locked --bin clio' or set CLIO_BIN." >&2
  exit 2
fi
printf '%s\n' "mcp-budget-check: using $bin" >&2

if ! tmp="$(mktemp)"; then
  printf '%s\n' "mcp-budget-check: cannot create a temporary file for the pack" >&2
  exit 2
fi
trap 'rm -f "$tmp"' EXIT

# Export first and check its status. The temp file is pack data, never executed.
if ! "$bin" mcp schema-export >"$tmp"; then
  printf '%s\n' "mcp-budget-check: clio mcp schema-export failed" >&2
  exit 2
fi

args=(--pack "$tmp" --preset-bin "$bin")
if [[ -n "${CLIO_SKILL_ENTRY_POINT:-}" ]]; then
  args+=(--skill "$CLIO_SKILL_ENTRY_POINT")
fi

# The tokenizer version is pinned in one file; a run with any other version
# could measure different numbers, so refuse it instead of comparing.
requirements="$root/benchmarks/mcp_token_budget/requirements.txt"
if [[ ! -r "$requirements" ]]; then
  printf '%s\n' "mcp-budget-check: cannot read the tokenizer pin file: $requirements" >&2
  exit 2
fi
pin="$(sed -n 's/^tiktoken==\([^[:space:]]*\).*$/\1/p' "$requirements" | head -n 1)"
if [[ -z "$pin" ]]; then
  printf '%s\n' "mcp-budget-check: cannot parse the tiktoken pin from $requirements" >&2
  exit 2
fi
if ! installed="$("$python" -c 'import tiktoken; print(tiktoken.__version__)' 2>/dev/null)"; then
  printf '%s\n' "mcp-budget-check: cannot import tiktoken with $python; install $requirements" >&2
  exit 2
fi
if [[ "$installed" != "$pin" ]]; then
  printf '%s\n' "mcp-budget-check: tiktoken $installed does not match the pinned $pin from $requirements" >&2
  exit 2
fi

# Run the checker in this shell so the temp pack file is always removed.
status=0
"$python" "$root/benchmarks/mcp_token_budget/budget_check.py" "${args[@]}" || status=$?

rm -f "$tmp"
trap - EXIT
exit "$status"
