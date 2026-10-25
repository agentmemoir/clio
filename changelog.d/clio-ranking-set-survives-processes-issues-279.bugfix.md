`clio ranking set` now saves the ranking environment to `cli-state.json` beside `deployment.json` in the clio config directory, so the next `clio ranking get` in a fresh process reports the environment you set instead of the defaults.

The command's text now says the change is saved for later commands and no longer claims it lasts only for the process.

A set that cannot be saved — no config directory resolves, or the write fails — now exits non-zero with an error instead of reporting `applied: true` for a change that would be lost when the process exited; `--dry-run` still writes nothing.

A missing or corrupt state file is ignored, so an existing installation boots on the built-in defaults exactly as before, and the MCP server's own ranking patches stay process-local.
