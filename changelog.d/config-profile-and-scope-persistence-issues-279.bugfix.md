`clio config use` now saves the profile it activated to `cli-state.json` beside `deployment.json` in the clio config directory, and `clio config set --scope profile` saves the profile-scoped value there too, so a later `clio config profiles` reports the active profile and a `config set --scope profile` in a later process is accepted instead of failing for want of one.

A boot restores the saved profile the same way `config use` applies it, puts the saved profile-scoped values back on top, and applies the saved ranking environment last, so an explicit `clio ranking set` still wins over the profile's own ranking.

A saved profile that no longer exists is skipped rather than failing the boot: the boot comes up with no active profile and still applies the rest of the saved state, and a missing or older state file behaves exactly as before.

`clio config set --scope session` now exits 1 with an `invalid_argument` error saying the value would apply to the running process only, be discarded when the command ends, and be unreadable by a later command, and pointing at `--scope deployment` for a value that persists; the scope the flag falls back to is the same one, so the command help and examples no longer show a session-scoped write.

The MCP `config_profile_apply` and `ranking_env_set` descriptions now say the change applies to the server process for its lifetime and is not written by the tool, while the CLI persists the same profile or ranking environment for later commands.

`clio config set --scope profile` now refuses a path the effective-config layer treats as secret, before applying or writing anything: `cli-state.json` seals nothing, so such a value could not be saved there and would be dropped. The command exits 1 with an `invalid_argument` error saying the value cannot be saved in the unsealed CLI state file at the profile scope and pointing at `--scope deployment`, whose deployment file seals values. The MCP `config_set` tool keeps accepting the same value at profile scope, because its overlay is process memory.

`clio config set --scope deployment` now exits 1 with `no_config_dir` when no deployment config file resolves, instead of reporting the value as set while writing nothing.
