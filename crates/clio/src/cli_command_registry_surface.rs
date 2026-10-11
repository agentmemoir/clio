// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the reserved
//! surfaces: the single commands `version`, `health`, `help`, `status`,
//! `setup`, `install`, and `uninstall`, and the `mcp` and `ops` subcommands.
//!
//! ## Owns
//! - The fifteen `CommandSpec` records and the `SURFACE` tranche that lists
//!   them; a two-word record name carries its parent group, so no separate
//!   parent table exists
//! - The help prose of the `version`, `health`, and `mcp schema-export`
//!   records: the notes their surfaces' `--help` prints with the record's
//!   usage line
//! - The setup record's pinned usage line, which names every flag its parser
//!   accepts, including the per-service flags the wizard constructs from the
//!   service role names; the record's `documented` entries are that accepted
//!   set, and `parse_setup_args` reads them
//! - The two child modules: the `compose up` and `compose down` records
//!   (`compose`) and the accepted-name derivation the reserved surfaces parse
//!   with (`flags`); attached here because the derivation reads these records
//!   through `super`, keeping the tranche and its derivation in one unit
//!
//! ## Does not own
//! - The record model or its rendering (see `cli_command_spec`)
//! - The registry and the shared global flags (see `cli_command_registry`)
//! - The surfaces' own dispatch and the prose each surface prints beside its
//!   record (see `cli_top_level`, `status_cli`, `setup_cli`, `install_cli`,
//!   `mcp_cli`, `ops_cli`, and `compose_cli`)
//!
//! ## Boundary
//!
//! Changes to one of these commands' flags, usage line, or summary text should
//! normally require changes here. Changes to how a surface dispatches should
//! not.
//!
//! Keep this module focused on the reserved-surface records.
//
// Each record carries the bytes its authored rows held: the summary is the
// catalog row, the usage line is the registered usage text, and the notes are
// the prose the surface's own help printed, so both rendered surfaces keep
// their exact output.

use crate::cli_command_registry::{GLOBAL_BACKEND, GLOBAL_BANK, GLOBAL_DB, GLOBAL_OUTPUT};
use crate::cli_command_spec::{CommandSpec, Flag, Positional, Section};

/// The globals a reserved surface accepts without printing: the shared parser
/// accepts every declared global on every surface, so each record it gates
/// that documents none of them lists all seven here. `help` and `setup` list
/// none: their dispatches read only their documented flags.
pub(crate) const RESERVED_GLOBALS: &[&str] = &[
    "db", "backend", "bank", "actor", "output", "no-input", "yes",
];

/// The globals `status` accepts without printing: its usage line already
/// documents `--db`, `--backend`, and `--output`.
const STATUS_UNDOCUMENTED_GLOBALS: &[&str] = &["bank", "actor", "no-input", "yes"];

/// The required `--harness` flag of `install` and `uninstall`.
const HARNESS_REQUIRED: Flag = Flag {
    required: true,
    ..Flag::text("harness", "<name>", "Harness `install_cli` resolves.")
};

/// The `--json` flag of `help`, the one boolean flag the help dispatch reads.
const HELP_JSON: Flag = Flag::toggle("json", "Selects the JSON catalog `cli_top_level` prints.");

/// The `help` line: the flag comes before the topic it narrows, which the
/// assembled order cannot write, so the line is pinned.
const HELP_USAGE: &str = "clio help [--json] [TOPIC]";

/// The `setup` line: the wizard's flag grammar keeps its own parser, and its
/// usage text names the whole accepted set, so the line is pinned.
const SETUP_USAGE: &str = "clio setup [--database managed|sqlite|existing] [--config PATH] [--db-path PATH] [--database-url ADDRESS] [--database-connect pasted|typed] [--database-host H] [--database-port P] [--database-name N] [--database-user U] [--database-password P] [--database-ssl MODE] [--database-ssl-rootcert PATH] [--database-option NAME=VALUE]... [--embed local|online] [--embed-url U] [--embed-model M] [--embed-key K] [--embed-provider tei|openai] [--embed-dims N] [--rerank local|online|skip] [--rerank-url U] [--rerank-model M] [--rerank-key K] [--rerank-provider tei|cohere] [--extract local|skip|online] [--compose-dir DIR] [--key-store file|keystore] [--sync-store file|keystore] [--allow-untested] [--yes] [--non-interactive] [--interactive]";

/// The optional `TOPIC` positional of `help`, pinned inside its usage line.
const TOPIC: Positional = Positional {
    label: "TOPIC",
    required: false,
    description: "",
};

/// The declared globals a record that documents `--db`, `--backend`, and
/// `--bank` still accepts without printing.
const DB_BANK_UNDOCUMENTED_GLOBALS: &[&str] = &["actor", "output", "no-input", "yes"];

/// The trio the `mcp` and `ops` subcommands print: the global value flags
/// every one of them names on its usage line.
const DB_BACKEND_BANK: &[Flag] = &[GLOBAL_DB, GLOBAL_BACKEND, GLOBAL_BANK];

/// The optional `ADDR` value of `mcp http`'s `--bind`: bare, the flag is
/// present with no value and the transport scans for a free port.
const BIND: Flag = Flag {
    value_optional: true,
    ..Flag::text("bind", "ADDR", "Bind address `mcp_cli` scans or uses.")
};

/// The flags `mcp http` documents past the database trio.
const TOKEN_FILE: Flag = Flag::text("token-file", "PATH", "Credential store `mcp_cli` loads.");
const SESSIONS: Flag = Flag::toggle("sessions", "Enables `mcp_cli` http sessions.");

/// The flags `mcp schema-export` documents; `--path` is the accepted alias
/// its handler resolves in place of `--out`.
const SCHEMA_EXPORT_OUT: Flag = Flag::text("out", "FILE", "Pack file `mcp_cli` writes.");
const SCHEMA_EXPORT_PATH: Flag = Flag::text("path", "FILE", "Alias of `--out` for the pack.");

/// The flags the mutating `ops` subcommands document past the database trio.
const DRY_RUN_FALSE: Flag = Flag::text("dry-run", "false", "Pass `false` to record or apply.");
const CONFIRM: Flag = Flag::toggle("confirm", "Confirms a live run in `ops_cli`.");
const REPAIR_PLAN: Flag = Flag::text("plan", "PLAN_ID", "Plan id `ops_cli`'s repair resolves.");
const REPAIR_ACTIONS: Flag = Flag::text("actions", "a,b", "Comma list `ops_cli`'s repair applies.");
const REINDEX_TARGET: Flag = Flag::choice(
    "target",
    "dense|lexical|all",
    "Index legs `ops_cli`'s reindex rebuilds.",
    &["dense", "lexical", "all"],
);

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
    examples: &[],
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
    examples: &[],
    tool: None,
    section: Section::Reserved,
};

/// `clio help`: the catalog screen and its topics; `--json` selects the
/// machine-readable catalog.
pub(crate) const HELP: CommandSpec = CommandSpec {
    name: "help",
    aliases: &[],
    summary: "Global flags: --db --backend --bank --actor --output (-o). Output mode: an explicit `--output`/`-o` always wins; otherwise text on a terminal (TTY) and JSON when piped. Forms: `--output json`, `--output=json`, `-o json`, `-o=json`. Empty values: a value flag given an empty value (`--bank ''`) is refused on every command; leave the flag off to take the default. Use -- after the command before positional text that starts with -; keep flags before it. Exit codes: 0 ok, 1 domain or runtime failure, 2 usage error or documented command refusal (ops repair or backup restore), 3 is a command-specific degraded outcome (clio ops reindex, backup prune or backup create --keep, or backup restore). Process stops caused by SIGPIPE, SIGINT, or SIGTERM cannot report a CLI error; shells that use the 128+signal convention show these as 141, 130, or 143, outside the command exit codes",
    notes: &[],
    positionals: &[TOPIC],
    documented: &[HELP_JSON],
    accepted_extra: &[],
    flag_aliases: &[],
    usage_override: Some(HELP_USAGE),
    usage_alternates: &[],
    examples: &[],
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
    examples: &[],
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
    examples: &[],
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
    examples: &[],
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
    examples: &[],
    tool: None,
    section: Section::Reserved,
};

/// `clio mcp schema-export`: print or write the tool schema pack.
pub(crate) const MCP_SCHEMA_EXPORT: CommandSpec = CommandSpec {
    name: "mcp schema-export",
    aliases: &[],
    summary: "Print/write the tool_schema pack (alias: --path)",
    notes: &[
        "--out FILE writes the pack to a file; --path is an accepted alias.\nWith neither flag the pack is written to stdout.",
    ],
    positionals: &[],
    documented: &[SCHEMA_EXPORT_OUT, SCHEMA_EXPORT_PATH],
    accepted_extra: RESERVED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
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
    documented: DB_BACKEND_BANK,
    accepted_extra: DB_BANK_UNDOCUMENTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
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
    ],
    accepted_extra: DB_BANK_UNDOCUMENTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: None,
    section: Section::Write,
};

/// `clio ops diagnose`: the read-only install and database health snapshot.
pub(crate) const OPS_DIAGNOSE: CommandSpec = CommandSpec {
    name: "ops diagnose",
    aliases: &[],
    summary: "Read-only install/database health snapshot",
    notes: &[],
    positionals: &[],
    documented: DB_BACKEND_BANK,
    accepted_extra: DB_BANK_UNDOCUMENTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: None,
    section: Section::Read,
};

/// `clio ops verify`: the referential-integrity and index-coverage checks.
pub(crate) const OPS_VERIFY: CommandSpec = CommandSpec {
    name: "ops verify",
    aliases: &[],
    summary: "Referential-integrity and index-coverage checks",
    notes: &[],
    positionals: &[],
    documented: DB_BACKEND_BANK,
    accepted_extra: DB_BANK_UNDOCUMENTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: None,
    section: Section::Read,
};

/// `clio ops doctor`: compose a repair plan without touching the store.
pub(crate) const OPS_DOCTOR: CommandSpec = CommandSpec {
    name: "ops doctor",
    aliases: &[],
    summary: "Compose a repair plan (read-only)",
    notes: &[],
    positionals: &[],
    documented: &[GLOBAL_DB, GLOBAL_BACKEND, GLOBAL_BANK, DRY_RUN_FALSE],
    accepted_extra: DB_BANK_UNDOCUMENTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: None,
    section: Section::Read,
};

/// `clio ops repair`: apply allowlisted repairs, live only with `--confirm`.
pub(crate) const OPS_REPAIR: CommandSpec = CommandSpec {
    name: "ops repair",
    aliases: &[],
    summary: "Apply allowlisted repairs (live run requires --confirm)",
    notes: &[],
    positionals: &[],
    documented: &[
        GLOBAL_DB,
        GLOBAL_BACKEND,
        GLOBAL_BANK,
        REPAIR_PLAN,
        REPAIR_ACTIONS,
        CONFIRM,
    ],
    accepted_extra: DB_BANK_UNDOCUMENTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: None,
    section: Section::Write,
};

/// `clio ops reindex`: rebuild derived indexes, live only with `--confirm`.
pub(crate) const OPS_REINDEX: CommandSpec = CommandSpec {
    name: "ops reindex",
    aliases: &[],
    summary: "Rebuild derived indexes (live run requires --confirm); a lexical rebuild replaces legacy plaintext index rows with keyed term digests. Ops exit codes: 0 healthy or completed, 1 domain/runtime failure or findings, 2 usage or repair refusal, 3 reindex failed",
    notes: &[],
    positionals: &[],
    documented: &[
        GLOBAL_DB,
        GLOBAL_BACKEND,
        GLOBAL_BANK,
        REINDEX_TARGET,
        DRY_RUN_FALSE,
        CONFIRM,
    ],
    accepted_extra: DB_BANK_UNDOCUMENTED_GLOBALS,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: None,
    section: Section::Write,
};

/// The reserved-surface records: the seven single commands, then the eight
/// `mcp` and `ops` subcommands, each group in its catalog order.
pub(crate) static SURFACE: &[CommandSpec] = &[
    VERSION,
    HEALTH,
    HELP,
    STATUS,
    SETUP,
    INSTALL,
    UNINSTALL,
    MCP_SCHEMA_EXPORT,
    MCP_STDIO,
    MCP_HTTP,
    OPS_DIAGNOSE,
    OPS_VERIFY,
    OPS_DOCTOR,
    OPS_REPAIR,
    OPS_REINDEX,
];

/// The `compose up` and `compose down` records, kept beside the reserved
/// surfaces' records.
#[path = "cli_command_registry_surface_compose.rs"]
pub(crate) mod compose;

/// The accepted-name derivation the reserved surfaces' parsers gate on.
#[path = "cli_command_registry_surface_flags.rs"]
pub(crate) mod flags;

pub(crate) use compose::{COMPOSE_DOWN, COMPOSE_UP};
pub(crate) use flags::parse_flag_names;

#[cfg(test)]
#[path = "cli_command_registry_surface_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "cli_command_registry_surface_parse_tests.rs"]
mod parse_tests;
