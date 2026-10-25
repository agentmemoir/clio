// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the seven reserved
//! single commands: `version`, `health`, `help`, `status`, `setup`, `install`,
//! and `uninstall`.
//!
//! ## Owns
//! - The seven `CommandSpec` records and their usage examples
//! - The `help` record's pinned usage line and its optional `TOPIC`
//!   positional, which names the command whose help opens
//! - The `setup` record's pinned usage line; its documented flags are the
//!   accepted set `setup_flags::parse_setup_args` reads
//! - The required `--harness` flag `install` and `uninstall` share
//!
//! ## Does not own
//! - The `SURFACE` tranche that lists these records and the flag facts its
//!   children share (see `cli_command_registry_surface`)
//! - The record model or its rendering (see `cli_command_spec`)
//! - The registry and the shared global flags (see `cli_command_registry`)
//! - The surfaces' own dispatch (see `cli_top_level`, `status_cli`,
//!   `setup_cli`, and `install_cli`)
//!
//! ## Boundary
//!
//! Changes to one of these commands' flags, usage line, summary text, or
//! examples should normally require changes here. Changes to how a surface
//! dispatches should not.
//!
//! Keep this module focused on the seven single-command records.
//

use super::RESERVED_GLOBALS;
use crate::cli_command_registry::{GLOBAL_BACKEND, GLOBAL_DB, GLOBAL_OUTPUT};
use crate::cli_command_spec::{CommandSpec, Example, Flag, Positional, Section};

/// The globals `status` accepts without printing: its usage line already
/// documents `--db`, `--backend`, and `--output`.
const STATUS_UNDOCUMENTED_GLOBALS: &[&str] = &["bank", "actor", "no-input", "yes"];

/// The required `--harness` flag of `install` and `uninstall`.
const HARNESS_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::text("harness", "<name>", "Harness `install_cli` resolves.")
};

/// The `--json` flag of `help`, one of the two boolean flags the help dispatch
/// reads.
const HELP_JSON: Flag = Flag::toggle("json", "Selects the JSON catalog `cli_top_level` prints.");

/// The `--markdown` flag of `help`: whole-manual generation instead of a
/// screen. It combines with no other output mode and takes no `TOPIC`.
const HELP_MARKDOWN: Flag = Flag::toggle(
    "markdown",
    "Writes the whole Markdown manual (one page per leaf command plus an index) into `--out DIR` instead of printing; exclusive with `--json` and with a TOPIC.",
);

/// The `--out` directory flag of `help`: mandatory with `--markdown`, refused
/// without it. The record marks no flag `required`, because `clio help` alone
/// still runs, so the conditional requirement is stated here and enforced by
/// `cli_top_level::markdown_request`.
const HELP_OUT: Flag = Flag::text(
    "out",
    "DIR",
    "Names the directory `--markdown` writes the manual into; required with `--markdown`, refused without it.",
);

/// The `help` line: the flags come before the topic they narrow, which the
/// assembled order cannot write, so the line is pinned.
const HELP_USAGE: &str = "clio help [--json] [--markdown --out DIR] [TOPIC]";

/// The `setup` line: the wizard's flag grammar keeps its own parser, and its
/// usage text names the whole accepted set, so the line is pinned.
const SETUP_USAGE: &str = "clio setup [--database managed|sqlite|existing] [--config PATH] [--db-path PATH] [--database-url ADDRESS] [--database-connect pasted|typed] [--database-host H] [--database-port P] [--database-name N] [--database-user U] [--database-password P] [--database-ssl MODE] [--database-ssl-rootcert PATH] [--database-option NAME=VALUE]... [--embed local|online] [--embed-url U] [--embed-model M] [--embed-key K] [--embed-provider tei|openai] [--embed-dims N] [--rerank local|online|skip] [--rerank-url U] [--rerank-model M] [--rerank-key K] [--rerank-provider tei|cohere] [--extract local|skip|online] [--compose-dir DIR] [--key-store file|keystore] [--sync-store file|keystore] [--allow-untested] [--yes] [--non-interactive] [--interactive]";

/// The optional `TOPIC` positional of `help`, pinned inside its usage line.
const TOPIC: Positional = Positional {
    label: "TOPIC",
    required: false,
    description: "Names a known command to open its help for, a `group verb` topic included; \
                  with no topic the whole catalog prints.",
};

/// `clio version`: the package identity line, over no flags and no tool.
pub(crate) const VERSION: CommandSpec = CommandSpec {
    name: "version",
    aliases: &[],
    summary: "Print package identity",
    notes: &["Prints the package name and version. Takes no flags."],
    positionals: &[],
    documented: &[],
    accepted_extra: RESERVED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio version",
            comment: Some("Print the package name and version."),
        },
        Example {
            command: "clio version --help",
            comment: Some("Show the record's own help."),
        },
    ],
    tool: None,
    section: Section::Reserved,
};

/// `clio health`: the machine-readable health line, over no flags and no tool.
pub(crate) const HEALTH: CommandSpec = CommandSpec {
    name: "health",
    aliases: &[],
    summary: "Machine-readable health/version JSON",
    notes: &[
        "Prints the machine-readable health JSON: name and version only.\nIt opens no database and checks no service.",
    ],
    positionals: &[],
    documented: &[],
    accepted_extra: RESERVED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio health",
            comment: Some("Print the machine-readable health JSON."),
        },
        Example {
            command: "clio health --help",
            comment: Some("Show the record's own help."),
        },
    ],
    tool: None,
    section: Section::Reserved,
};

/// `clio help`: the catalog screen and its topics; `--json` selects the
/// machine-readable catalog and `--markdown --out DIR` the whole manual.
pub(crate) const HELP: CommandSpec = CommandSpec {
    name: "help",
    aliases: &[],
    summary: "Global flags: --db --backend --bank --actor --output (-o). Output mode: an explicit `--output`/`-o` always wins; otherwise text on a terminal (TTY) and JSON when piped. Forms: `--output json`, `--output=json`, `-o json`, `-o=json`. Empty values: a value flag given an empty value (`--bank ''`) is refused on every command; leave the flag off to take the default. Use -- after the command before positional text that starts with -; keep flags before it. Exit codes: 0 ok, 1 domain or runtime failure, 2 usage error or documented command refusal (ops repair or backup restore), 3 is a command-specific degraded outcome (clio ops reindex, backup prune or backup create --keep, or backup restore). 75 means the command could not report a clean success and that nothing was lost or invalid, and the command's own message says whether a retry helps: a storage lock conflict is cleared by retrying, and a restore that left a derived difference is not. Process stops caused by SIGPIPE, SIGINT, or SIGTERM cannot report a CLI error; shells that use the 128+signal convention show these as 141, 130, or 143, outside the command exit codes",
    notes: &[],
    positionals: &[TOPIC],
    documented: &[HELP_JSON, HELP_MARKDOWN, HELP_OUT],
    accepted_extra: &[],
    flag_aliases: &[],
    usage_override: Some(HELP_USAGE),
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio help",
            comment: Some("Print the catalog screen."),
        },
        Example {
            command: "clio help backup",
            comment: Some("Open one group's help from the catalog."),
        },
        Example {
            command: "clio help --json",
            comment: Some("Print the machine-readable catalog."),
        },
        Example {
            command: "clio help --markdown --out ./manual",
            comment: Some("Write the whole Markdown manual into ./manual."),
        },
    ],
    tool: None,
    section: Section::Reserved,
};

/// `clio status`: read-only stack health over the injected probes.
pub(crate) const STATUS: CommandSpec = CommandSpec {
    name: "status",
    aliases: &[],
    summary: "Read-only unified stack health (setup, database, listeners, Compose, diagnose). Exit codes: 0 healthy, 1 degraded, 2 usage",
    notes: &[],
    positionals: &[],
    documented: &[
        GLOBAL_DB,
        GLOBAL_BACKEND,
        Flag::text("dir", "DIR", "Compose root `status_cli` probes."),
        GLOBAL_OUTPUT,
    ],
    accepted_extra: STATUS_UNDOCUMENTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio status",
            comment: Some("Probe the configured stack and print the verdict."),
        },
        Example {
            command: "clio status -o json",
            comment: Some("Emit the report as JSON."),
        },
        Example {
            command: "clio status --dir ./deploy/compose",
            comment: Some("Point the Compose probe at one root."),
        },
    ],
    tool: None,
    section: Section::Read,
};

/// `clio setup`: the wizard that writes the deployment config. Its flag
/// grammar keeps its own parser, the record's `documented` entries are the
/// accepted set that parser reads, and the pinned line names the whole set.
pub(crate) const SETUP: CommandSpec = CommandSpec {
    name: "setup",
    aliases: &[],
    summary: "Configure the stack and write the deployment config the binary reads (see `clio setup --help` for the full flag set)",
    notes: &[],
    positionals: &[],
    documented: crate::setup_flags::ACCEPTED,
    accepted_extra: &[],
    flag_aliases: &[],
    usage_override: Some(SETUP_USAGE),
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio setup --database sqlite --db-path ./memory.db --embed local --rerank skip --non-interactive",
            comment: Some(
                "Answer every question for a SQLite store, local embedding, and no reranker, then run without prompting.",
            ),
        },
        Example {
            command: "clio setup --database sqlite --db-path ./memory.db --embed skip --rerank skip --non-interactive",
            comment: Some(
                "Configure only the SQLite store, with no embedding or reranking service.",
            ),
        },
        Example {
            command: "clio setup --database sqlite --db-path ./memory.db --embed local --rerank skip --config ./deployment.json --non-interactive",
            comment: Some(
                "Write the deployment config to an explicit path instead of the default.",
            ),
        },
    ],
    tool: None,
    section: Section::Reserved,
};

/// `clio install`: register the clio MCP server with an agent harness.
pub(crate) const INSTALL: CommandSpec = CommandSpec {
    name: "install",
    aliases: &[],
    summary: "Register the clio MCP server with a supported agent harness (see `clio install --help` for the supported harnesses)",
    notes: &[],
    positionals: &[],
    documented: &[HARNESS_REQUIRED],
    accepted_extra: RESERVED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio install --harness claude",
            comment: Some("Register the MCP server with Claude Code."),
        },
        Example {
            command: "clio install --harness codex",
            comment: Some("Register with Codex."),
        },
        Example {
            command: "clio install --harness antigravity",
            comment: Some("Register with Antigravity."),
        },
    ],
    tool: None,
    section: Section::Reserved,
};

/// `clio uninstall`: remove the clio MCP server registration.
pub(crate) const UNINSTALL: CommandSpec = CommandSpec {
    name: "uninstall",
    aliases: &[],
    summary: "Remove the clio MCP server registration from a supported agent harness (see `clio uninstall --help` for the supported harnesses)",
    notes: &[],
    positionals: &[],
    documented: &[HARNESS_REQUIRED],
    accepted_extra: RESERVED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio uninstall --harness claude",
            comment: Some("Remove the Claude Code registration."),
        },
        Example {
            command: "clio uninstall --harness codex",
            comment: Some("Remove the Codex registration."),
        },
        Example {
            command: "clio uninstall --harness antigravity",
            comment: Some("Remove the Antigravity registration."),
        },
    ],
    tool: None,
    section: Section::Reserved,
};

#[cfg(test)]
#[path = "cli_command_registry_surface_singles_tests.rs"]
mod tests;
