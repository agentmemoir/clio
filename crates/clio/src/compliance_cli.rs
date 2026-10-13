// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio compliance` CLI surface: the local
//! read-only scope ledger that `clio compliance inventory` renders.
//!
//! ## Owns
//! - `clio compliance inventory` argument decoding, store-fact measurement,
//!   path resolution, and exit codes
//! - The subcommand list derivation and the group/subcommand help screens
//! - The text view: one line per row plus the complete flag and the
//!   unclassified list (the JSON view is the serialized `ScopeInventory`)
//!
//! ## Does not own
//! - The ledger's rows and semantics (see `clio_compliance`)
//! - The subcommand's static record (see `cli_command_registry_compliance`)
//! - Store measurement (`clio-store`), path resolution (`clio-config`), the
//!   output/error contracts (see `cli_output` / `cli_error`), or the argument
//!   grammar (see `cli_args`)
//!
//! ## Boundary
//!
//! Changes to a compliance verb's flags, usage text, or summary should normally
//! require changes in `cli_command_registry_compliance`; changes to its
//! dispatch, decoding, or views should normally require changes here. Changes
//! to the ledger's semantics should not require changes here.
//!
//! Keep this module focused on the `clio compliance` surface.

use crate::cli_args::{self, FlagSpec, ParsedArgs};
use crate::cli_error::{CliError, EXIT_OK, EXIT_USAGE};
use crate::cli_output::{self, OutputMode};
use crate::cli_read::{CliIo, emit, fail};
use clio_compliance::{
    Custody, InventoryFacts, InventoryInput, InventoryPaths, InventoryRow, RetentionAction,
    ScopeInventory, scope_inventory,
};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Dispatch `clio compliance <subcommand>` with the process environment,
/// streams, and current directory.
pub(crate) fn compliance(args: Vec<String>) -> ExitCode {
    let env = |key: &str| std::env::var(key).ok();
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    let mut input = std::io::stdin();
    let mut io = CliIo {
        out: &mut out,
        err: &mut err,
        stdin: &mut input,
        is_tty: cli_output::stdout_is_tty(),
        no_color: cli_output::no_color_requested(),
        env: &env,
    };
    // The resolver needs a current directory to absolute-ize a relative
    // fallback; failing to learn one is operational, not a usage error.
    let current_dir = match std::env::current_dir() {
        Ok(dir) => Ok(dir),
        Err(error) => Err(current_dir_error(&error)),
    };
    run(&mut io, args, current_dir)
}

/// The operational error for a current-directory lookup that failed.
fn current_dir_error(error: &std::io::Error) -> CliError {
    CliError::operational(
        "internal",
        format!("cannot determine the current directory: {error}"),
    )
}

/// Resolve, parse, and run one `compliance` subcommand.
///
/// `current_dir` is the injected seam: the real entry resolves the process
/// current directory, tests inject one (or an error) directly.
pub(crate) fn run(
    io: &mut CliIo,
    args: Vec<String>,
    current_dir: Result<PathBuf, CliError>,
) -> ExitCode {
    let fallback = cli_output::mode_from_raw(&args, io.is_tty);
    let mut tokens = args.into_iter();
    let Some(sub) = tokens.next() else {
        let _ = cli_output::write_text(&mut io.out, &group_help_text());
        return ExitCode::from(EXIT_USAGE);
    };
    if matches!(sub.as_str(), "--help" | "-h" | "help") {
        let _ = cli_output::write_text(&mut io.out, &group_help_text());
        return ExitCode::from(EXIT_OK);
    }
    if !compliance_subcommands().contains(&sub.as_str()) {
        return fail(io, fallback, &unknown_subcommand(&sub));
    }
    let collected: Vec<String> = tokens.collect();
    // The scan runs before the parser so a help request is not turned into a
    // usage error by a stray token or an unknown flag; `retention_cli` answers
    // its subcommands' help the same way.
    if collected
        .iter()
        .any(|arg| matches!(arg.as_str(), "help" | "--help" | "-h"))
    {
        let _ = cli_output::write_text(&mut io.out, &compliance_help(&sub));
        return ExitCode::from(EXIT_OK);
    }
    match sub.as_str() {
        "inventory" => run_inventory(io, fallback, collected, current_dir),
        // The derived list above already refused every other spelling; the
        // fallback keeps the arm-free case a refusal rather than a silent run.
        other => fail(io, fallback, &unknown_subcommand(other)),
    }
}

/// The `compliance` subcommands: the registered `compliance` records' first
/// token past the prefix, in registry order. `pub(crate)` so the registry tests
/// assert this derivation's output instead of rebuilding it.
///
/// The split is optional: a one-token suffix is the whole subcommand, and a
/// longer name contributes only its first token, so the derived list can
/// neither skip a record nor carry a mangled name.
pub(crate) fn compliance_subcommands() -> Vec<&'static str> {
    crate::cli_command_registry::names()
        .filter_map(|name| name.strip_prefix("compliance "))
        .map(|rest| rest.split_once(' ').map_or(rest, |(sub, _)| sub))
        .collect()
}

/// The `clio compliance` subcommands, one usage line each.
///
/// This is the screen `clio compliance --help` and `clio help compliance`
/// print; a subcommand's own flags stay behind `clio compliance <subcommand>
/// --help`.
fn group_help_text() -> String {
    let mut lines: Vec<String> = Vec::new();
    for sub in compliance_subcommands() {
        lines.extend(crate::cli_help_surfaces::wrapped_usage(
            &format!("compliance {sub}"),
            78,
            0,
        ));
    }
    lines.push(
        "Ask a subcommand for its flags with `clio compliance <subcommand> --help`.".to_owned(),
    );
    lines.join("\n")
}

/// One compliance subcommand's help, read from the shared usage registry.
fn compliance_help(sub: &str) -> String {
    let command = format!("compliance {sub}");
    let mut text = crate::cli_help_surfaces::wrapped_usage(&command, 78, 0).join("\n");
    if !text.contains('\n') {
        text.push('\n');
    }
    let notes = crate::cli_command_registry::find(&command).map_or(&[][..], |spec| spec.notes);
    for note in notes {
        text.push('\n');
        text.push_str(note);
        text.push('\n');
    }
    text
}

/// The error for an unknown subcommand, suggesting the closest verb.
///
/// The candidates are the derived subcommand list, so a verb added to the
/// registry is both dispatched and suggested, with no second list to update.
fn unknown_subcommand(sub: &str) -> CliError {
    let candidates = compliance_subcommands();
    let hint = cli_args::suggest(sub, &candidates).map_or_else(
        || "try `clio compliance inventory`".to_owned(),
        |best| format!("did you mean `clio compliance {best}`?"),
    );
    CliError::usage(format!("unknown `compliance` subcommand `{sub}`")).with_hint(hint)
}

/// Run `compliance inventory`: parse the record's flags, build the report, and
/// emit it. Exit 0 whenever a report was produced, even an incomplete one.
///
/// Peer enablement is not readable from the CLI, so the report's `complete`
/// field stays `false` from this command.
fn run_inventory(
    io: &mut CliIo,
    fallback: OutputMode,
    tokens: Vec<String>,
    current_dir: Result<PathBuf, CliError>,
) -> ExitCode {
    let parsed = match cli_args::parse(&inventory_spec(), tokens)
        .map_err(|error| crate::cli_error::with_help_hint(error, "compliance inventory"))
    {
        Ok(parsed) => parsed,
        Err(error) => return fail(io, fallback, &error),
    };
    let mode = match cli_output::resolve_output(parsed.value("output"), io.is_tty) {
        Ok(mode) => mode,
        Err(error) => return fail(io, fallback, &error),
    };
    if let Err(error) = crate::runtime_cli::no_positionals(&parsed) {
        return fail(io, mode, &error);
    }
    let current_dir = match current_dir {
        Ok(dir) => dir,
        Err(error) => return fail(io, mode, &error),
    };
    match inventory_report(io, &parsed, &current_dir) {
        Ok(report) => {
            let payload = match serde_json::to_value(&report) {
                Ok(payload) => payload,
                Err(error) => {
                    return fail(
                        io,
                        mode,
                        &CliError::operational(
                            "internal",
                            format!("scope inventory did not serialize: {error}"),
                        ),
                    );
                }
            };
            emit(io, mode, &payload, &render_text(&report));
            ExitCode::from(EXIT_OK)
        }
        Err(error) => fail(io, mode, &error),
    }
}

/// The flags `compliance inventory` accepts: the record's own parser spec — so
/// the accepted flags and the usage text cannot drift — plus the globals every
/// parser spec merges (which is where `-o` comes from).
fn inventory_spec() -> FlagSpec {
    match crate::cli_command_registry::find("compliance inventory") {
        Some(spec) => spec.parser_spec(),
        None => FlagSpec::new(&[], &[]),
    }
}

/// Build the read-only scope ledger for the resolved target.
///
/// The store is measured through the one read-only API each backend owns, and
/// the local paths come from the resolvers the other commands use. Nothing is
/// written: the ledger reads the store and the resolved paths and records the
/// declared copies without touching them.
fn inventory_report(
    io: &mut CliIo,
    parsed: &ParsedArgs,
    current_dir: &Path,
) -> Result<ScopeInventory, CliError> {
    let get = |key: &str| (io.env)(key);
    let target =
        crate::db_target::resolve_target(&get, parsed.value("db"), parsed.value("backend"))
            .map_err(CliError::usage)?;
    let facts = facts_for(&target)?;
    let paths = InventoryPaths {
        db_path: store_path(&target),
        kms_path: clio_config::kms_path::resolve_kms_path(&get, &target.url).unwrap_or_default(),
        backup_dir: clio_config::resolve_backup_dir(&get, current_dir).path,
    };
    let input = InventoryInput {
        facts,
        paths,
        declared_copies: parsed.values_of("copy").iter().map(PathBuf::from).collect(),
        // The configuration surface exposes no cheap peer-enablement read, so
        // the ledger receives `None` and its peer row stays open rather than
        // claiming a scope this command cannot verify.
        peers_enabled: None,
    };
    Ok(scope_inventory(&input))
}

/// The live-store facts, measured read-only, exactly as the backup commands
/// choose a backend: a SQLite target resolves to its file, a Postgres target
/// to its database.
fn facts_for(target: &clio_config::ResolvedDatabase) -> Result<InventoryFacts, CliError> {
    match target.backend.as_str() {
        "postgres" => clio_store::postgres_target_state(&target.url)
            .map(InventoryFacts::Postgres)
            .map_err(|error| CliError::from_am_error(&error)),
        "sqlite" if target.url == "sqlite::memory:" => Err(in_memory_refusal()),
        "sqlite" => clio_store::sqlite_file_state(Path::new(&target.url))
            .map(InventoryFacts::Sqlite)
            .map_err(|error| CliError::from_am_error(&error)),
        other => Err(unsupported_backend(other)),
    }
}

/// The SQLite store file the ledger's path field names (its sidecar row reads
/// it), or the empty placeholder a Postgres target carries: the database is
/// not a file.
fn store_path(target: &clio_config::ResolvedDatabase) -> PathBuf {
    if target.backend == "sqlite" {
        PathBuf::from(&target.url)
    } else {
        PathBuf::new()
    }
}

/// The refusal for `sqlite::memory:`, whose database dies with the process that
/// opened it, so a fresh process has no store on disk to inventory.
fn in_memory_refusal() -> CliError {
    CliError::usage(
        "`clio compliance inventory` reads a store file; `sqlite::memory:` keeps the database in \
         this process and there is nothing on disk to inventory",
    )
    .with_hint("point `--db` at a SQLite database file")
}

/// The refusal for a backend this build cannot measure.
fn unsupported_backend(backend: &str) -> CliError {
    CliError::usage(format!(
        "`clio compliance inventory` does not support the {backend} backend"
    ))
    .with_hint(format!(
        "backend must be sqlite or postgres, got `{backend}`"
    ))
}

/// The text view: one line per row (family, source, link or gap, custody,
/// action, open flag), then the complete flag and the unclassified list.
fn render_text(report: &ScopeInventory) -> String {
    let mut lines: Vec<String> = report.rows.iter().map(row_line).collect();
    lines.push(format!("complete: {}", report.complete));
    lines.push(format!(
        "unclassified: {}",
        if report.unclassified.is_empty() {
            "none".to_owned()
        } else {
            report.unclassified.join(", ")
        }
    ));
    lines.join("\n")
}

/// One row's line: the subject link when it exists, else the named gap.
fn row_line(row: &InventoryRow) -> String {
    let link = row
        .subject_link
        .as_deref()
        .or(row.gap.as_deref())
        .unwrap_or("-");
    format!(
        "family={} source={} link={link} custody={} action={} open={}",
        row.family,
        row.source,
        custody_label(row.custody),
        action_label(row.retention_action),
        row.open
    )
}

/// The text spelling of a custody, kept beside the ledger's own serde spelling
/// so both surfaces read the same word.
fn custody_label(custody: Custody) -> &'static str {
    match custody {
        Custody::Clio => "clio",
        Custody::Operator => "operator",
        Custody::Caller => "caller",
        Custody::Peer => "peer",
        Custody::Unknown => "unknown",
    }
}

/// The text spelling of a retention action, kept beside the ledger's own serde
/// spelling so both surfaces read the same word.
fn action_label(action: RetentionAction) -> &'static str {
    match action {
        RetentionAction::Erase => "erase",
        RetentionAction::Regenerate => "regenerate",
        RetentionAction::Prune => "prune",
        RetentionAction::Retain => "retain",
        RetentionAction::Record => "record",
        RetentionAction::Open => "open",
    }
}

#[cfg(test)]
#[path = "compliance_cli_tests.rs"]
mod compliance_cli_tests;
