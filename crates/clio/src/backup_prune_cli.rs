// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for `clio backup prune`: parsing the keep count,
//! applying the confirmation rule, and showing what a prune kept, removed, and
//! left alone. It is also the one implementation `create --keep N` calls.
//!
//! ## Owns
//! - The `prune` flags (`--keep`, `--confirm`, `--dry-run`) and their views
//! - The keep-count parsing and its refusals, shared with `create --keep`
//! - The JSON field set (`kept`, `deleted`, `already_gone`, `failed`,
//!   `unreadable`, and the byte totals) and the matching text view
//! - The exit codes: 0 for a report and for a complete prune, 3 when a
//!   confirmed prune could not remove every candidate
//!
//! ## Does not own
//! - Choosing the keep set and the candidates (see `clio-backup`'s `prune`)
//! - Removing a candidate and measuring the bytes (see `clio-backup`'s
//!   `prune_delete`)
//! - The confirmation flag's meaning (see `cli_confirm`) and directory
//!   resolution (see `backup_cli`)
//!
//! ## Boundary
//!
//! Changes to `clio backup prune`'s flags, views, or exit codes should normally
//! require changes here. Changes to which archives are selected or how one is
//! removed should not require changes here.
//!
//! Keep this module focused on the `prune` command.

use crate::backup_cli::resolve_backup_dir;
use crate::cli_args::{self, FlagSpec, ParsedArgs};
use crate::cli_error::{CliError, EXIT_OK, EXIT_PARTIAL};
use crate::cli_help_usage;
use crate::cli_output::{self, OutputMode};
use crate::cli_read::{CliIo, emit, fail};
use clio_backup::prune::{self, PruneRefusal};
use clio_backup::prune_delete::{CandidateOutcome, CandidateState, PruneReport};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The value flags `prune` adds to the globals: the one count it takes.
const VALUE_FLAGS: &[&str] = &["keep"];

/// The boolean flags `prune` adds to the globals: the confirmation gate and the
/// explicit report-only spelling. Neither prompts, so the command behaves the
/// same on a terminal and in a script.
const BOOL_FLAGS: &[&str] = &["confirm", "dry-run"];

/// What one prune produced, for the caller that embeds it in a larger report.
pub(crate) struct KeepOutcome {
    /// The JSON report, under the caller's own key.
    pub payload: Value,
    /// The text report, appended to the caller's own lines.
    pub text: String,
    /// The exit code this prune earns on its own.
    pub exit: u8,
}

/// Run `clio backup prune` against injected streams, env, and cwd.
pub(crate) fn run(
    io: &mut CliIo,
    fallback: OutputMode,
    tokens: impl IntoIterator<Item = String>,
    current_dir: Result<PathBuf, CliError>,
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
        let usage = cli_help_usage::usage_for("backup prune")
            .unwrap_or_else(|| "usage: clio backup prune".to_owned());
        let _ = cli_output::write_text(&mut io.out, &usage);
        return ExitCode::from(EXIT_OK);
    }
    let mode = match cli_output::resolve_output(parsed.value("output"), io.is_tty) {
        Ok(mode) => mode,
        Err(error) => return fail(io, fallback, &error),
    };
    if let Err(error) = crate::runtime_cli::no_positionals(&parsed) {
        return fail(io, mode, &error);
    }
    if let Err(error) = refuse_store_flags(&parsed) {
        return fail(io, mode, &error);
    }
    let keep = match required_keep(&parsed) {
        Ok(keep) => keep,
        Err(error) => return fail(io, mode, &error),
    };
    let get = |key: &str| (io.env)(key);
    let backup = resolve_backup_dir(&get, &current_dir);
    let report = match prune_backup_dir(&backup.path, keep, removes(&parsed)) {
        Ok(report) => report,
        Err(error) => return fail(io, mode, &error),
    };
    emit(io, mode, &payload(&report), &render(&report));
    ExitCode::from(exit_code(&report))
}

/// The flags `backup prune` accepts.
fn spec() -> FlagSpec {
    FlagSpec::new(VALUE_FLAGS, BOOL_FLAGS)
}

/// The keep count `prune` requires, refusing a missing or unusable one.
///
/// `prune` has nothing to do without the count, so a missing `--keep` is a usage
/// error rather than a default: a default would be a policy this command does
/// not have.
fn required_keep(parsed: &ParsedArgs) -> Result<usize, CliError> {
    keep_count(parsed)?.ok_or_else(|| {
        CliError::usage("`clio backup prune` needs `--keep N`").with_hint(
            "`--keep` is how many of the newest snapshots to keep, for example `--keep 3`",
        )
    })
}

/// The `--keep` count, or `None` when the flag was not given.
///
/// This is the one place the count is read, so `clio backup prune` and
/// `clio backup create --keep` refuse exactly the same values for exactly the
/// same reasons. `--keep 0` is refused because the newest snapshot is never a
/// candidate; a value that is not a count is refused because a prune that
/// guessed a count would delete the wrong number of snapshots.
pub(crate) fn keep_count(parsed: &ParsedArgs) -> Result<Option<usize>, CliError> {
    let Some(raw) = parsed.value("keep") else {
        return Ok(None);
    };
    let count = raw.parse::<usize>().map_err(|_| {
        CliError::usage(format!("`--keep` takes a count, got `{raw}`")).with_hint("try `--keep 3`")
    })?;
    if count == 0 {
        return Err(
            CliError::usage(prune::refusal_message(PruneRefusal::KeepZero)).with_hint(
                "`--keep` is how many of the newest snapshots to keep, and the newest \
                        snapshot is never deleted",
            ),
        );
    }
    Ok(Some(count))
}

/// Whether this run removes anything.
///
/// The rule is the one every destructive command in the product already uses:
/// `--confirm` (or the global `--yes`) removes, `--dry-run` never does, and
/// neither prompts. A run with neither flag is a report, which is the safe
/// reading of a destructive command whose flag was forgotten.
fn removes(parsed: &ParsedArgs) -> bool {
    crate::cli_confirm::confirmed(parsed) && !parsed.has("dry-run")
}

/// The prune `create --keep N` runs after a successful create.
///
/// It is the same function the `prune` verb calls, so the two cannot disagree
/// about what a keep count selects or what a confirmation means. The caller's
/// own confirmation flags are read, because `create` inherits them.
pub(crate) fn run_keep(
    keep: usize,
    parsed: &ParsedArgs,
    backup_dir: &Path,
) -> Result<KeepOutcome, CliError> {
    let report = prune_backup_dir(backup_dir, keep, removes(parsed))?;
    Ok(KeepOutcome {
        payload: payload(&report),
        text: render(&report),
        exit: exit_code(&report),
    })
}

/// One prune of one backup directory: list it, select, then assess or delete.
///
/// A missing backup directory is an empty listing rather than a failure, exactly
/// as it is for `list`, so pruning a directory nobody has created yet says
/// nothing and exits 0.
///
/// The listing is read once, then each entry is examined once without following
/// a link, and that answer goes into the selection: this is the only place that
/// touches the filesystem for a prune, so the two halves of a prune cannot
/// disagree about which entries are links.
fn prune_backup_dir(backup_dir: &Path, keep: usize, remove: bool) -> Result<PruneReport, CliError> {
    let rows = match clio_backup::list_snapshot_rows(backup_dir, None) {
        Ok(rows) => rows,
        Err(error) if error.code == clio_types::ErrorCode::NotFound => Vec::new(),
        Err(error) => return Err(CliError::from_am_error(&error)),
    };
    let links = clio_backup::LinkedPaths::among(&rows);
    let plan = prune::select(&rows, &links, keep).map_err(|refusal| {
        CliError::usage(prune::refusal_message(refusal)).with_hint("`--keep` must be at least 1")
    })?;
    Ok(if remove {
        clio_backup::prune_delete::delete(backup_dir, &plan)
    } else {
        clio_backup::prune_delete::assess(backup_dir, &plan)
    })
}

/// The exit code a report earns.
///
/// 0 for a report-only run and for a confirmed run that removed every
/// candidate. 3 for a confirmed run that could not: the run is a degraded
/// outcome, not a usage refusal and not a plain success, so a script can tell it
/// from both. A skipped candidate (a symlink, a path outside the directory) is
/// not a failure: it was never a legitimate target, and the report says so.
fn exit_code(report: &PruneReport) -> u8 {
    if report.has_failures() {
        EXIT_PARTIAL
    } else {
        EXIT_OK
    }
}

/// Refuse the store flags a prune cannot honor.
///
/// A prune reads archives, never a store, so `--db` and `--backend` would be
/// silently ignored: the keep count applies to the backup directory whatever
/// store the operator is pointing at.
fn refuse_store_flags(parsed: &ParsedArgs) -> Result<(), CliError> {
    for flag in ["db", "backend"] {
        if parsed.value(flag).is_some() {
            return Err(CliError::usage(format!(
                "`clio backup prune` reads archives, not a store; `--{flag}` cannot change which \
                 snapshots it selects"
            ))
            .with_hint("drop the flag, or use `clio backup create --db` to take a snapshot"));
        }
    }
    Ok(())
}

/// The JSON prune: what was kept, what happened to every candidate, and the
/// bytes.
///
/// The five list fields are stable, so a script can read the one it cares about
/// without parsing prose, and the two byte figures never overlap:
/// `reclaimable_bytes` is what the candidates occupied when the directory was
/// listed, `reclaimed_bytes` is what this run really unlinked.
fn payload(report: &PruneReport) -> Value {
    let mut candidates = Vec::new();
    let mut deleted = Vec::new();
    let mut already_gone = Vec::new();
    let mut failed = Vec::new();
    let mut skipped = Vec::new();
    for outcome in &report.candidates {
        let entry = entry(outcome);
        candidates.push(entry.clone());
        match outcome.state {
            CandidateState::Deleted => deleted.push(entry),
            CandidateState::AlreadyGone => already_gone.push(entry),
            CandidateState::Failed => failed.push(entry),
            CandidateState::Symlink | CandidateState::OutsideDirectory => skipped.push(entry),
            // A pending candidate is already in `candidates`: claiming it was
            // removed would be the one lie this report must never tell.
            CandidateState::Pending => {}
        }
    }
    json!({
        "ok": !report.has_failures(),
        "keep": report.keep,
        "dry_run": report.dry_run,
        "confirmed": !report.dry_run,
        "backup_dir": report.backup_dir.display().to_string(),
        "kept": report.kept,
        "candidates": candidates,
        "deleted": deleted,
        "already_gone": already_gone,
        "failed": failed,
        "skipped": skipped,
        "unreadable": report.unreadable,
        "reclaimable_bytes": report.reclaimable_bytes,
        "reclaimed_bytes": report.reclaimed_bytes,
        "remaining_bytes": report.remaining_bytes,
    })
}

/// One candidate as a JSON object, with its state, its store, and its bytes.
///
/// The same object appears in `candidates` and in whichever of the four outcome
/// lists it belongs to, so a script reads one shape. `reason` is present only
/// when there is a reason to give: a skip or a failure.
fn entry(outcome: &CandidateOutcome) -> Value {
    let mut value = json!({
        "id": outcome.id,
        "path": outcome.path.display().to_string(),
        "created_at": outcome.created_at,
        "store": store_label(outcome),
        "state": outcome.state,
        "listed_bytes": outcome.listed_bytes,
        "reclaimed_bytes": outcome.reclaimed_bytes,
    });
    if let Some(reason) = &outcome.detail {
        value["reason"] = json!(reason);
    }
    value
}

/// The store an archive belongs to, as `backend/slug`.
///
/// A backup directory may legitimately hold several stores' snapshots, so every
/// report line names the store its archive came from: an operator reading a
/// mixed directory can see whose snapshot a line is about.
fn store_label(outcome: &CandidateOutcome) -> String {
    format!("{}/{}", outcome.backend, outcome.slug)
}

/// The human prune: the directory, then one line per kept archive, per
/// candidate, and per unreadable archive, then the byte totals.
fn render(report: &PruneReport) -> String {
    let verb = if report.dry_run {
        "report only; nothing was removed"
    } else {
        "removed"
    };
    let mut lines = vec![format!(
        "prune {} --keep {} ({verb})",
        report.backup_dir.display(),
        report.keep
    )];
    lines.push(format!("  kept ({})", report.kept.len()));
    for kept in &report.kept {
        lines.push(format!(
            "    {} {}/{} · {} B · {}",
            kept.archive.id,
            kept.archive.backend,
            kept.archive.slug,
            kept.archive.listed_bytes,
            keep_reason(kept.reason)
        ));
    }
    lines.push(format!("  candidates ({})", report.candidates.len()));
    for outcome in &report.candidates {
        let detail = outcome
            .detail
            .as_deref()
            .map_or_else(String::new, |detail| format!(" · {detail}"));
        lines.push(format!(
            "    {} {}/{} · {} · {} B{detail}",
            outcome.id,
            outcome.backend,
            outcome.slug,
            outcome.state.label(),
            outcome.listed_bytes
        ));
    }
    if !report.unreadable.is_empty() {
        lines.push(format!("  unreadable ({})", report.unreadable.len()));
        for archive in &report.unreadable {
            lines.push(format!(
                "    {} · {} B · {}",
                archive.id, archive.listed_bytes, archive.reason
            ));
        }
    }
    lines.push(format!(
        "  reclaimable {} B · reclaimed {} B · remaining {} B",
        report.reclaimable_bytes, report.reclaimed_bytes, report.remaining_bytes
    ));
    lines.join("\n")
}

/// The human wording for the rule that kept an archive.
fn keep_reason(reason: prune::KeepReason) -> &'static str {
    match reason {
        prune::KeepReason::KeepWindow => "newest",
        prune::KeepReason::LastOfStore => "the only snapshot of this store",
    }
}

#[cfg(test)]
#[path = "backup_prune_cli_tests.rs"]
mod tests;

/// The report's exact JSON shape and its exit codes, in both output modes.
#[cfg(test)]
#[path = "backup_prune_cli_golden_tests.rs"]
mod golden_tests;

/// The ways a prune refuses: a bad keep count, an unreadable archive, a
/// symlinked archive, and a removal that failed.
#[cfg(test)]
#[path = "backup_prune_cli_failures_tests.rs"]
mod failures_tests;
