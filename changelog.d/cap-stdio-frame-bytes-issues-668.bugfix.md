The stdio MCP transport now refuses a frame over 10 MiB with a clear invalid-data error naming the cap, instead of growing memory without bound while it waits for a newline.
