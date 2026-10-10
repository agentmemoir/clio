# Clio

## Install / Upgrade

```bash
curl -fsSL https://github.com/agentmemoir/clio/releases/latest/download/install.sh | sh
```

## Setup

| Mode | Requirement |
| --- | --- |
| 100% Offline | Docker required for database, embeddings, reranking, and RFT |
| Dependency Free | Configure embedding and reranking API keys |
| Customizable | Fully customizable for advanced users |

### Offline Mode

Run:

```bash
clio setup
```

For fully offline use, choose a local database:

- **SQLite** — Recommended for most users; no Docker required for the database.
- **PostgreSQL** — Better for read/write-heavy workloads; automatically managed by Clio in Docker or use an existing PostgreSQL instance.


## Configure Your Agent

Clio speaks MCP. One command registers Clio as an MCP server in your agent:

```bash
clio install --harness "OpenCode"
clio uninstall --harness "OpenCode"
```

Supported: `claude`, `codex`, `opencode`, `antigravity`, `hermes`, `cursor`, `github copilot`

Both commands are safe to run twice. They print what they did, and `uninstall` removes only Clio's entry. The server runs `clio mcp stdio`, so it uses the database from `clio setup`.

Restart your agent afterwards. Then ask it to list its MCP tools: `store`, `retrieve`, and `compose_context` must appear.


## Uninstall

```bash
rm ~/.local/bin/clio
rm -r ~/.local/share/clio
```

> AI agents must not refactor this file without explicit approval from the operator.
