// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the three `clio mcp`
//! subcommands: `schema-export`, `stdio`, and `http`.
//!
//! ## Owns
//! - The three `CommandSpec` records and their usage examples
//! - The `schema-export` flag-alias pair (`path` is the accepted alias of
//!   `out`) and both documented spellings the pair names
//! - The `http` record's `--bind`, `--token-file`, `--sessions`, and
//!   `--insecure-no-auth` flags, and the database trio both transports print
//! - The `--tools` flag both transports and the export document, and the
//!   usage refusal a bad value raises
//! - The `--discovery` switch both transports document
//!
//! ## Does not own
//! - The `SURFACE` tranche and the shared flag facts its children read (see
//!   `cli_command_registry_surface`)
//! - The record model or its rendering (see `cli_command_spec`)
//! - The registry and the shared global flags (see `cli_command_registry`)
//! - The `mcp` dispatch and the transports themselves (see `mcp_cli` and the
//!   `clio-mcp` crate)
//!
//! ## Boundary
//!
//! Changes to one of these commands' flags, usage line, summary text, or
//! examples should normally require changes here. Changes to how a transport
//! serves should not.
//!
//! Keep this module focused on the three `mcp` records.
//

use super::{DB_BANK_UNDOCUMENTED_GLOBALS, RESERVED_GLOBALS};
use crate::cli_command_registry::{GLOBAL_BACKEND, GLOBAL_BANK, GLOBAL_DB};
use crate::cli_command_spec::{CommandSpec, Example, Flag, Section};

/// The optional `ADDR` value of `mcp http`'s `--bind`: bare, the flag is
/// present with no value and the transport scans for a free port.
const BIND: Flag = Flag {
    value_optional: true,
    ..Flag::text("bind", "ADDR", "Bind address `mcp_cli` scans or uses.")
};

/// The tool-selection flag both transports document: the preset names, the
/// `custom:` group list, and the setting that decides when no flag is given.
const TOOLS: Flag = Flag::text(
    "tools",
    "PRESET",
    "Tool groups the server advertises: `minimal`, `coding`, `ops`, `all`, or custom:GROUP[,GROUP] for the minimal set plus the listed groups and what they require. With no --tools the config setting mcp.tool_profile applies; when that is unset the minimal preset is selected.",
);

/// The discovery switch both transports document: whether the three discovery
/// tools are advertised.
const DISCOVERY: Flag = Flag::choice(
    "discovery",
    "on|off",
    "Discovery tools `mcp_cli` advertises; with no flag the config setting mcp.discovery applies.",
    &["on", "off"],
);

/// The database trio both transports document, plus the two selections.
const DB_BACKEND_BANK_TOOLS: &[Flag] = &[GLOBAL_DB, GLOBAL_BACKEND, GLOBAL_BANK, TOOLS, DISCOVERY];

/// The flags `mcp http` documents past the database trio.
const TOKEN_FILE: Flag = Flag::text("token-file", "PATH", "Credential store `mcp_cli` loads.");
const SESSIONS: Flag = Flag::toggle("sessions", "Enables `mcp_cli` http sessions.");
const INSECURE_NO_AUTH: Flag = Flag::toggle(
    "insecure-no-auth",
    "Serves `mcp_cli` http without a credential.",
);
const INSECURE_BIND_ANY: Flag = Flag::toggle(
    "insecure-bind-any",
    "Lets `--insecure-no-auth` bind a non-loopback address.",
);

/// The flags `mcp schema-export` documents; `--path` is the accepted alias
/// its handler resolves in place of `--out`.
const SCHEMA_EXPORT_OUT: Flag = Flag::text("out", "FILE", "Pack file `mcp_cli` writes.");
const SCHEMA_EXPORT_PATH: Flag = Flag::text("path", "FILE", "Alias of `--out` for the pack.");

/// The tool selection the export documents: the preset names, the `custom:`
/// group list, and the full catalog a pack with no flag holds.
const SCHEMA_EXPORT_TOOLS: Flag = Flag::text(
    "tools",
    "PRESET",
    "Tool groups the pack holds: `minimal`, `coding`, `ops`, `all`, or custom:GROUP[,GROUP] for the minimal set plus the listed groups and what they require. With no --tools the pack holds the whole catalog.",
);
const SCHEMA_EXPORT_FLAGS: &[Flag] = &[SCHEMA_EXPORT_OUT, SCHEMA_EXPORT_PATH, SCHEMA_EXPORT_TOOLS];

/// `clio mcp schema-export`: print or write the tool schema pack.
pub(crate) const MCP_SCHEMA_EXPORT: CommandSpec = CommandSpec {
    name: "mcp schema-export",
    aliases: &[],
    summary: "Print/write the tool_schema pack (alias: --path)",
    notes: &[
        "--out FILE writes the pack to a file; --path is an accepted alias.\nWith neither flag the pack is written to stdout.",
        "--tools narrows the pack to a tool-group selection: minimal, coding, ops, all, or custom:GROUP[,GROUP] for the minimal set plus the listed groups and what they require. With no --tools the pack holds every tool in the catalog.",
    ],
    positionals: &[],
    documented: SCHEMA_EXPORT_FLAGS,
    accepted_extra: RESERVED_GLOBALS,
    flag_aliases: &[("path", "out")],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio mcp schema-export",
            comment: Some("Print the tool schema pack to stdout."),
        },
        Example {
            command: "clio mcp schema-export --out ./tools.json",
            comment: Some("Write the pack to a file."),
        },
        Example {
            command: "clio mcp schema-export --path ./tools.json",
            comment: Some("Write through the accepted alias spelling."),
        },
    ],
    tool: None,
    section: Section::Reserved,
};

/// `clio mcp stdio`: the stdio transport; its help prose is the shared
/// database block the surface prints beside the record's usage line.
pub(crate) const MCP_STDIO: CommandSpec = CommandSpec {
    name: "mcp stdio",
    aliases: &[],
    summary: "Run the MCP stdio transport",
    notes: &[],
    positionals: &[],
    documented: DB_BACKEND_BANK_TOOLS,
    accepted_extra: DB_BANK_UNDOCUMENTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio mcp stdio",
            comment: Some("Serve the stdio transport over the default store."),
        },
        Example {
            command: "clio mcp stdio --db ./memory.db",
            comment: Some("Serve one SQLite store."),
        },
        Example {
            command: "clio mcp stdio --db ./memory.db --bank default",
            comment: Some("Scope the session to one bank."),
        },
    ],
    tool: None,
    section: Section::Write,
};

/// `clio mcp http`: the Streamable HTTP transport; `--bind` is the one flag
/// whose value may be omitted, and a bare `--bind` means the unbound
/// auto-scan the transport resolves.
pub(crate) const MCP_HTTP: CommandSpec = CommandSpec {
    name: "mcp http",
    aliases: &[],
    summary: "Run the MCP Streamable HTTP transport; with no flag (or a bare --bind) scan 127.0.0.1:{range} for the first free port; an explicit HOST:PORT binds exactly that address",
    notes: &[],
    positionals: &[],
    documented: &[
        BIND,
        GLOBAL_DB,
        GLOBAL_BACKEND,
        GLOBAL_BANK,
        TOKEN_FILE,
        SESSIONS,
        INSECURE_NO_AUTH,
        INSECURE_BIND_ANY,
        TOOLS,
        DISCOVERY,
    ],
    accepted_extra: DB_BANK_UNDOCUMENTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio mcp http --bind 127.0.0.1:8765",
            comment: Some("Bind exactly one address."),
        },
        Example {
            command: "clio mcp http --bind 127.0.0.1:8765 --db ./memory.db",
            comment: Some("Bind and serve one SQLite store."),
        },
        Example {
            command: "clio mcp http --bind 127.0.0.1:8765 --sessions",
            comment: Some("Turn on HTTP sessions."),
        },
    ],
    tool: None,
    section: Section::Write,
};

#[cfg(test)]
#[path = "cli_command_registry_surface_mcp_tests.rs"]
mod tests;
