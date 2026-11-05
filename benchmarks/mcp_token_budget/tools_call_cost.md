# tools/call result cost

One fixed client session holds one `initialize` exchange and one `tools/call` exchange for `ranking_env_get`, served over stdio with an in-memory SQLite store, so every number below describes the same call sequence on both sides of the change.
The session before the change measures 1904 bytes across its two response lines and the session after it measures 1050, a reduction of 854 bytes.
The tool payload itself is 774 bytes, and it appears twice in the before response, once as the structured result and once as a text copy, while the after response carries it once.
The numbers come from the two committed transcripts, `crates/clio-mcp/tests/fixtures/tools_call_session_before.json` and `crates/clio-mcp/tests/fixtures/tools_call_session.json`, and the cost record is regenerated with:

```
python3 benchmarks/mcp_token_budget/measure_transcript.py --before crates/clio-mcp/tests/fixtures/tools_call_session_before.json --after crates/clio-mcp/tests/fixtures/tools_call_session.json --out benchmarks/mcp_token_budget/baseline/tools_call_cost.json
```

Two runs of that command on the same fixtures write byte-identical files.
Token counts are not measured on this host: the pinned tokenizer needs the `tiktoken` package, which is not installed and cannot be installed here, so the token rows carry a null value and the label `Not measured` instead of an estimate.
