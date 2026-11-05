`tools/list` now advertises only the tools of the profile the configuration selects, instead of the whole bound catalog.
The profile is set with the new `mcp.tool_profile` key, one of `minimal`, `coding`, `ops`, or `all`, and it is `minimal` when the key is absent, so a client that never sets it sees a shorter list than before; `all` keeps the previous full list.
A tool outside the selected profile stays callable and its arguments are still validated against the full catalog, so only the advertised surface shrinks.
An unknown profile name is refused with a message naming the names this build knows, rather than quietly selecting a different profile than the one written.
