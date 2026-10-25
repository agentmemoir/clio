// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio backup restore` CLI surface: its
//! flags, the blast radius and confirmation it shows, its text and JSON views,
//! and the exit codes it maps results to.
//!
//! ## Owns
//! - The `restore` views over its record's flags (`--latest`, `--yes`,
//!   `--no-pre-backup`, `--allow-missing-keys`, `--dry-run`; the accepted set
//!   is the `backup restore` registry record's) and the selector rule
//! - The confirmation prompt and the exit codes: 0 for a restore or a plan,
//!   3 for a partial, count-mismatched, or not-durable restore, and 75 for a
//!   restore whose only difference is a derived table (every user-data row
//!   loaded; retrying the restore does not clear it, and the index pipeline
//!   reconciles it on its next pass)
//!
//! ## Does not own
//! - The restore sequence and its refusals (see `backup_restore_flow`)
//! - The text and JSON views (see `backup_restore_report`)
//! - Extracting an archive or swapping a store (see `clio-backup` and
//!   `clio-store`)
//! - Directory, target, and snapshot resolution (see `backup_cli`)
//!
//! ## Boundary
//!
//! Changes to `clio backup restore`'s flags, prompt, or exit codes should
//! normally require changes here. Changes to the order of a restore or to how it
//! is worded should not.
//!
//! Keep this module focused on the `restore` command surface.

use crate::backup_restore_exit::{self, RestoreFailure};
use crate::backup_restore_flow::{
    self, Plan, PreRestore, RESULT_COUNT_MISMATCH, RESULT_DERIVED_DIFFERENT, RESULT_PARTIAL, Report,
};
use crate::backup_restore_report::{payload, render, render_plan};
use crate::cli_args::{self, FlagSpec, ParsedArgs};
use crate::cli_error::{CliError, EXIT_OK};
use crate::cli_help_usage;
use crate::cli_output::{self, OutputMode};
use crate::cli_read::{CliIo, emit, fail};
use clio_store::SpaceCheck;
use std::path::PathBuf;
use std::process::ExitCode;

/// Run `clio backup restore` against injected streams, env, and cwd.
pub(crate) fn run(
    io: &mut CliIo,
    fallback: OutputMode,
    tokens: impl IntoIterator<Item = String>,
    current_dir: Result<PathBuf, CliError>,
) -> ExitCode {
    run_with_probe(io, fallback, tokens, current_dir, &clio_store::DiskSpace)
}

/// [`run`] with the space check injected, so a test can refuse room without
/// filling a filesystem.
pub(crate) fn run_with_probe(
    io: &mut CliIo,
    fallback: OutputMode,
    tokens: impl IntoIterator<Item = String>,
    current_dir: Result<PathBuf, CliError>,
    check: &dyn SpaceCheck,
) -> ExitCode {
    let current_dir = match current_dir {
        Ok(dir) => dir,
        Err(error) => return fail(io, fallback, &error),
    };
    let parsed = match cli_args::parse(&spec(), tokens) {
        Ok(parsed) => parsed,
        Err(error) => return fail(io, fallback, &error),
    };
    if parsed.has("help") {
        let usage = cli_help_usage::help_for("backup restore")
            .unwrap_or_else(|| "usage: clio backup restore".to_owned());
        let _ = cli_output::write_text(&mut io.out, &usage);
        return ExitCode::from(EXIT_OK);
    }
    let mode = match cli_output::resolve_output(parsed.value("output"), io.is_tty) {
        Ok(mode) => mode,
        Err(error) => return fail(io, fallback, &error),
    };
    if let Err(error) = one_selector(&parsed) {
        return fail(io, mode, &error);
    }
    let env = io.env;
    let plan = match backup_restore_flow::plan(env, &parsed, &current_dir, check) {
        Ok(plan) => plan,
        Err(error) => return fail(io, mode, &error),
    };
    if plan.dry_run {
        let report = backup_restore_flow::planned(plan);
        emit(io, mode, &payload(&report), &render(&report));
        return ExitCode::from(EXIT_OK);
    }
    // The blast radius goes to stderr, the same stream the prompt reads, so
    // stdout stays the report a script can parse. It is printed in both modes:
    // a JSON consumer on a terminal still has to decide about the same store.
    let _ = cli_output::write_text(&mut io.err, &render_plan(&plan));
    if let Err(error) = confirm(io, &parsed, &plan) {
        return fail(io, mode, &error);
    }
    match backup_restore_flow::place(plan) {
        Ok(report) => {
            emit(io, mode, &payload(&report), &render(&report));
            ExitCode::from(exit_code(&report))
        }
        Err(error) => fail(io, mode, &error),
    }
}

/// The flags `backup restore` accepts: the `backup restore` record's own
/// flags, decoded through the registry, plus the globals every parser spec
/// merges.
fn spec() -> FlagSpec {
    match crate::cli_command_registry::find("backup restore") {
        Some(spec) => spec.parser_spec(),
        None => FlagSpec::new(&[], &[]),
    }
}

/// Exactly one selector: a snapshot id, or `--latest`.
fn one_selector(parsed: &ParsedArgs) -> Result<(), CliError> {
    let positionals = parsed.positionals().len();
    let latest = parsed.has("latest");
    if positionals > 1 || (positionals == 1 && latest) {
        return Err(backup_restore_exit::error(
            RestoreFailure::BadArguments,
            "usage",
            "`clio backup restore` takes one snapshot id, or --latest, not both",
        )
        .with_hint("try `clio backup restore --latest`"));
    }
    if positionals == 0 && !latest {
        return Err(backup_restore_exit::error(
            RestoreFailure::BadArguments,
            "usage",
            "`clio backup restore` needs a snapshot id, or --latest",
        )
        .with_hint("run `clio backup list` to see the ids"));
    }
    Ok(())
}

/// Ask for the snapshot id, unless `--yes` already answered.
///
/// A non-interactive run never prompts: it refuses and names the flag that
/// would let it proceed. The typed answer must equal the snapshot id exactly,
/// with only a trailing line ending removed.
fn confirm(io: &mut CliIo, parsed: &ParsedArgs, plan: &Plan) -> Result<(), CliError> {
    if parsed.has("yes") {
        return Ok(());
    }
    if !io.is_tty || parsed.has("no-input") {
        return Err(confirmation_required());
    }
    let prompt = prompt_text(
        &plan.snapshot_id,
        &plan.target.display,
        &plan.pre_restore,
        plan.key.is_some(),
    );
    let mut input = std::io::BufReader::new(&mut *io.stdin);
    match crate::compose_cli::prompt_line(true, &prompt, &mut io.err, &mut input) {
        Some(answer) if strip_newline(&answer) == plan.snapshot_id.as_str() => Ok(()),
        Some(_) => Err(backup_restore_exit::error(
            RestoreFailure::BadArguments,
            "usage",
            format!(
                "the typed id did not match snapshot `{}`; nothing was restored",
                plan.snapshot_id
            ),
        )
        .with_hint("type the snapshot id exactly, or pass `--yes` to restore without a prompt")),
        None => Err(confirmation_required()),
    }
}

/// The one question a confirmation asks, worded for the run in front of the
/// operator: what is replaced, that a merge is not on offer, whether the
/// archive's key store is replaced with it, and whether there is a way back.
///
/// Every clause after the first is conditional, because `Plan` already knows
/// which of them applies to this run. An unconditional "the pre-restore
/// snapshot is the way back" is false under `--no-pre-backup`, and false again
/// when the target is missing or measured empty, and the operator reads it at the
/// moment they decide.
pub(crate) fn prompt_text(
    snapshot_id: &str,
    target: &str,
    pre_restore: &PreRestore,
    key_replaced: bool,
) -> String {
    let mut clauses = vec!["the current store is replaced, not merged".to_owned()];
    if key_replaced {
        clauses.push("the archive's key store replaces the live one".to_owned());
    }
    clauses.push(match pre_restore.reason {
        Some(_) => "no pre-restore snapshot will be taken, so there is no way back".to_owned(),
        None => "a pre-restore snapshot is taken first and is the way back".to_owned(),
    });
    format!(
        "restore {snapshot_id} over {target}? {}; type the snapshot id to proceed:",
        clauses.join(", and ")
    )
}

/// The refusal for a restore that cannot ask: no terminal and no `--yes`.
fn confirmation_required() -> CliError {
    backup_restore_exit::error(
        RestoreFailure::BadArguments,
        "usage",
        "a terminal is required to confirm a restore; pass `--yes` to restore without a prompt",
    )
    .with_hint("run it on a terminal, or pass `--yes`")
}

/// The typed answer with only one trailing line ending removed.
fn strip_newline(answer: &str) -> &str {
    answer
        .strip_suffix("\r\n")
        .or_else(|| answer.strip_suffix('\n'))
        .unwrap_or(answer)
}

/// The process exit code one report maps to. A partial restore or a failed
/// user-data count check is not success; a rename that could not be fsynced is
/// a degraded restore and uses the command-specific exit code 3. A run whose
/// only difference is a derived table loaded every user-data row and lost
/// nothing, so it takes the temporary code rather than either of those, and the
/// report's own derived-difference text says a retry does not clear it.
fn exit_code(report: &Report) -> u8 {
    let condition = if report.result == RESULT_PARTIAL || report.result == RESULT_COUNT_MISMATCH {
        RestoreFailure::PartialOrCountMismatch
    } else if report.result == RESULT_DERIVED_DIFFERENT {
        RestoreFailure::DerivedTableDiffers
    } else if report.directory_synced == Some(false) {
        RestoreFailure::RestoredNotDurable
    } else {
        RestoreFailure::RestoredOrPlanned
    };
    backup_restore_exit::exit_code_for(condition)
}

#[cfg(test)]
#[path = "backup_restore_cli_tests.rs"]
mod tests;

/// The restore surface's early-refusal and fsync-warning edges.
#[cfg(test)]
#[path = "backup_restore_cli_edges_tests.rs"]
mod edge_tests;

/// The security acceptance condition: no content, subject id, or key byte in
/// any restore output.
#[cfg(test)]
#[path = "backup_restore_cli_security_tests.rs"]
mod backup_restore_cli_security_tests;

/// The restore surface's refusal edges: a repeated member name, an open store
/// handle held across the restore, which of two simultaneous refusals is
/// reported, an unwritable target directory, an empty-but-initialized target,
/// and the two hostile-manifest refusals.
#[cfg(test)]
#[path = "backup_restore_cli_refusal_tests.rs"]
mod backup_restore_cli_refusal_tests;

/// The golden-output cases: the full text plan, the full text report, the
/// full JSON payload, and a full text-mode refusal, pinned byte for byte
/// against a hand-built archive with a fixed id and stamps.
#[cfg(test)]
#[path = "backup_restore_cli_golden_tests.rs"]
mod backup_restore_cli_golden_tests;

/// The restore's cross-process lock, end to end through the real command.
#[cfg(test)]
#[path = "backup_restore_cli_lock_e2e_tests.rs"]
mod backup_restore_cli_lock_e2e_tests;
