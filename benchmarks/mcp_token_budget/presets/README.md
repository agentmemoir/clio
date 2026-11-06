# Measured MCP preset surface

This directory holds the frozen record of the shipped MCP tool surface as four presets, each measured as the canonical bytes of the pack's `tools` array only and the cl100k_base tokens those bytes cost, with the pack envelope excluded so a preset row and the eager baseline row it names are the same quantity.

The token counts come from the repo-pinned offline tokenizer (`tiktoken==0.14.0`, `cl100k_base`), never from an estimate, and the four exact exports sit beside the record so every number can be recomputed without the binary.

| preset | tools | measured bytes | measured tokens | baseline row |
| --- | --- | --- | --- | --- |
| `minimal` | 6 | 17321 | 3652 | `client/synthetic-eager-client/turn/1` |
| `coding` | 18 | 34913 | 7436 | `client/synthetic-eager-client/turn/1` |
| `ops` | 22 | 33466 | 7181 | `client/synthetic-eager-client/turn/1` |
| `all` | 86 | 148633 | 31667 | `client/synthetic-eager-client/turn/1` |

Eager delivery is discovery off: every tool definition with its input schema is delivered at turn 1, and this build has no discovery mode, so no row here is a discovery-on row and no row may be quoted as one.

Revision caveat: the frozen baseline row of 105632 bytes / 21691 tokens was measured at catalog hash `fnv1a64:c0fc6a61cedf1299`, while these rows were measured at `fnv1a64:c68c036f893dc6de`, so a delta against 105632 mixes preset narrowing with catalog drift instead of isolating the preset, and the same-revision full surface is the `all` row.

Regenerate from the repository root with `python3 benchmarks/mcp_token_budget/preset_rows.py --bin target/release/clio --out benchmarks/mcp_token_budget/presets/frozen-v1.json`, which rewrites this record and the four packs from the binary's own exports.

The record is [frozen-v1.json](frozen-v1.json), its exact exports are in [packs/](packs/), and the baseline it pins is [the frozen baseline record](../baseline/frozen-v1.json).
