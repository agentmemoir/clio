`clio memtree query` and `clio memtree get` (and the MCP `memtree_query` / `memtree_get` tools) now read the stored MemTree rows instead of only the current process's in-memory tree, so a node written by an earlier command is found again in a later one.

A bank that holds no matching node is now reported as not found — exit code 1 with `"code":"not_found"` — instead of an empty success. `memtree get` for a node present in neither the store nor the tree reports `not_found` rather than `unknown_path`, so a caller that branched on the old code needs to match the new one.

The in-memory tree is still consulted for nodes this process attached and has not yet flushed, so a read issued immediately after a write keeps working.
