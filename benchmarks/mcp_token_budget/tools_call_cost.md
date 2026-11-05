# tools/call result cost

One fixed client session holds one `initialize` exchange and one `tools/call` exchange for `ranking_env_get`, served over stdio with an in-memory SQLite store, so every number below describes the same call sequence on both sides of the change. The session before the change measures 1904 bytes across its two response lines and the session after it measures 1050, a reduction of 854 bytes. The tool payload itself is 774 bytes, and it appears twice in the before response, once as the structured result and once as a text copy, while the after response carries it once. The numbers come from the two committed transcripts, `crates/clio-mcp/tests/fixtures/tools_call_session_before.json` and `crates/clio-mcp/tests/fixtures/tools_call_session.json`, and the cost record is regenerated with:

```
uv pip install --python 3.12 --target "$HOME/.cache/clio-mcp-token-budget" tiktoken==0.14.0
PYTHONPATH="$HOME/.cache/clio-mcp-token-budget" "$(uv python find 3.12)" benchmarks/mcp_token_budget/measure_transcript.py --before crates/clio-mcp/tests/fixtures/tools_call_session_before.json --after crates/clio-mcp/tests/fixtures/tools_call_session.json --out benchmarks/mcp_token_budget/baseline/tools_call_cost.json
```

Two runs of that command on the same fixtures write byte-identical files, and the record holds no host path, no timestamp, and no executable path.

The token rows are measured with `tiktoken` 0.14.0 on CPython 3.12.14, installed outside the repository so no dependency enters the tree: the two sessions hold 629 cl100k_base tokens before the change and 354 after, a reduction of 275 tokens. The record names that package, version, and interpreter, and it carries `session.delta.bytes` and `session.delta.tokens` rows beside the per-session rows, each labelled `Measured` with a digest over the two inputs it compares.

When the tokenizer package is missing, the same command still writes the record, and every token row then carries a null value, the label `Not measured`, and the reason.
