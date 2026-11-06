# Measured MCP preset surface

This directory holds the frozen record of the shipped MCP tool surface as four presets, each measured as the canonical bytes of the pack's `tools` array only and the cl100k_base tokens those bytes cost, with the pack envelope excluded so a preset row and the eager baseline row it names are the same quantity.

The token counts come from the repo-pinned offline tokenizer (`tiktoken==0.14.0`, `cl100k_base`), never from an estimate, and the four exact exports sit beside the record so every number can be recomputed without the binary.

| preset | tools | measured bytes | measured tokens | catalog hash | baseline row |
| --- | --- | --- | --- | --- | --- |
| `minimal` | 6 | 17321 | 3652 | `fnv1a64:61637a63ede0e509` | `client/synthetic-eager-client/turn/1` |
| `coding` | 18 | 34913 | 7436 | `fnv1a64:94f71ecd2a69bfe2` | `client/synthetic-eager-client/turn/1` |
| `ops` | 22 | 33466 | 7181 | `fnv1a64:8e0c2ec55c64f36d` | `client/synthetic-eager-client/turn/1` |
| `all` | 86 | 148729 | 31691 | `fnv1a64:2b74f4ad094e2424` | `client/synthetic-eager-client/turn/1` |

Eager delivery is the client mode of the frozen baseline record `benchmarks/mcp_token_budget/baseline/frozen-v1.json` whose client is `synthetic-eager-client`, and that mode delivers every tool definition with its input schema at turn 1, so that mode is the discovery-off surface for these rows.

The same baseline record's `synthetic-on-demand-client` is the discovery-on mode, and no row here uses it, so no row may be quoted as a discovery-on row.

Each pack carries its own catalog hash, recorded on the row measured from that pack, so the four rows need not share one revision: the `all` pack's hash `fnv1a64:2b74f4ad094e2424` is the one to compare against for a same-revision comparison with the `all` row, which is the full surface, and the frozen baseline row was measured at `fnv1a64:c0fc6a61cedf1299`.

Comparing any preset row with the frozen baseline row of 105632 bytes / 21691 tokens therefore mixes preset narrowing with catalog drift instead of isolating the preset, and the same-revision full surface is the `all` row.

Regenerate from the repository root with `python3 benchmarks/mcp_token_budget/preset_rows.py --bin target/release/clio --out benchmarks/mcp_token_budget/presets/frozen-v1.json`, which rewrites this record and the four packs from the binary's own exports.

The record is [frozen-v1.json](frozen-v1.json), its exact exports are in [packs/](packs/), and the baseline it pins is [the frozen baseline record](../baseline/frozen-v1.json).
