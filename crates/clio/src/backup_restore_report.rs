// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for how one restore is shown: the blast radius
//! the operator reads before confirming, the report they read afterwards, and
//! the JSON payload a script parses.
//!
//! ## Owns
//! - `render_plan`, the pre-confirmation blast radius
//! - `render` and `payload`, the text report and the JSON payload
//! - The one-line helpers they compose, including what happened to the key store
//!
//! ## Does not own
//! - The flags, the confirmation prompt, and the exit codes (see
//!   `backup_restore_cli`)
//! - The restore sequence and the refusals (see `backup_restore_flow`)
//!
//! ## Boundary
//!
//! Changes to how a restore is worded or shaped should normally require changes
//! here. Changes to the order a restore runs in should not.
//!
//! Keep this module focused on rendering one restore.

use crate::backup_restore_flow::{
    Counts, Plan, PreRestore, RESULT_COUNT_MISMATCH, RESULT_DERIVED_DIFFERENT, RESULT_PARTIAL,
    RESULT_PLANNED, RESULT_RESTORED, Report,
};
use crate::backup_restore_schema::Schema;
use crate::count_noun::count_noun;
use serde_json::{Value, json};
use std::path::PathBuf;

/// What a restore whose directory could not be fsynced did and did not achieve.
/// Named once so the text warning and the JSON field cannot drift apart.
const NOT_DURABLE: &str = "the target's directory could not be fsynced, so the rename is not \
                           durable against a crash right now";

/// The command that refills the two derived indexes a Postgres restore empties.
/// The truncate clears the vector table with the rest of the schema and loads no
/// rows back into it, so the vector and lexical index are empty afterwards and
/// this command derives both again from the content the restore placed. It does
/// not touch the work queue, so it clears no surplus there.
const REINDEX_COMMAND: &str = "clio ops reindex --target dense --confirm --backend postgres";

/// Where that command must be pointed, and what it needs to run. It is never
/// printed with the store address: on this backend `--db` is a URL that can
/// carry a password, so the line names the database the operator already
/// restored instead of repeating an address a report must not hold.
const REINDEX_NOTICE: &str =
    "point it at the restored database: pass the same `--db` this restore used, with an embedder \
     configured, because the dense leg fails closed without one";

/// The blast radius, before the operator is asked to confirm.
pub(super) fn render_plan(plan: &Plan) -> String {
    let mut lines = vec![format!(
        "restore plan: snapshot {} (created {})",
        plan.snapshot_id, plan.created_at
    )];
    lines.push(format!(
        "  target {} ({}, {})",
        plan.target.display, plan.target.backend, plan.target.database
    ));
    if let Some(schema) = &plan.store_schema {
        lines.push(format!("  store schema: {schema}"));
    }
    if plan.target_exists {
        lines.push(format!(
            "  current store: {}",
            counts_text(&plan.counts_before)
        ));
    } else {
        lines.push("  current store: none (the target store does not exist yet)".to_owned());
    }
    lines.push(format!(
        "  incoming archive: {}",
        counts_text(&plan.counts_incoming)
    ));
    lines.push(format!("  {}", schema_text(&plan.schema)));
    lines.push("  the store is replaced, not merged".to_owned());
    lines.push(match &plan.key {
        Some(placement) => format!(
            "  the key store at {} is replaced too",
            placement.destination.display()
        ),
        None => "  the key store is not part of this restore".to_owned(),
    });
    lines.push(format!("  {}", pre_restore_plan_text(&plan.pre_restore)));
    lines.join("\n")
}

/// The human report: the essentials, then the result.
pub(super) fn render(report: &Report) -> String {
    let mut lines = vec![format!(
        "restore {} over {} ({}, {})",
        report.snapshot_id, report.target.display, report.target.backend, report.target.database
    )];
    if let Some(schema) = &report.store_schema {
        lines.push(format!("  store schema: {schema}"));
    }
    lines.push(format!("  {}", schema_text(&report.schema)));
    lines.push(format!(
        "  counts before: {}",
        counts_text(&report.counts_before)
    ));
    if let Some(after) = &report.counts_after {
        lines.push(format!("  counts after: {}", counts_text(after)));
    }
    if let Some(incoming) = &report.counts_incoming {
        lines.push(format!("  incoming archive: {}", counts_text(incoming)));
    }
    lines.push("  the store is replaced, not merged".to_owned());
    lines.push(key_store_text(report));
    if let Some(command) = reindex_command(report) {
        lines.push(format!("  rebuild the derived index: {command}"));
        lines.push(format!("  {REINDEX_NOTICE}"));
    }
    if report.dry_run {
        lines.push(format!(
            "  would replace: {}",
            would_replace(report).join(", ")
        ));
    }
    if report.dry_run {
        lines.push(format!("  {}", pre_restore_plan_text(&report.pre_restore)));
    } else {
        lines.push(format!(
            "  {}",
            pre_restore_result_text(&report.pre_restore)
        ));
    }
    if report.directory_synced == Some(false) {
        lines.push(format!("  warning: {NOT_DURABLE}"));
    }
    lines.push(result_text(report));
    lines.join("\n")
}

/// The result line, including the failure a partial restore must carry.
fn result_text(report: &Report) -> String {
    match report.result {
        RESULT_PARTIAL | RESULT_COUNT_MISMATCH => {
            let step = report.failed_step.as_deref().unwrap_or("unknown");
            let mut text = format!("  result: {} (failed step: {step})", report.result);
            if let Some(failure) = &report.failure {
                text.push_str("\n  ");
                text.push_str(failure);
            }
            text
        }
        RESULT_DERIVED_DIFFERENT => {
            // No failed step and no failure: the restore did its job, and the
            // line says which rebuildable table says otherwise.
            let mut text = format!("  result: {}", report.result);
            if let Some(difference) = &report.derived_difference {
                text.push_str("\n  ");
                text.push_str(difference);
            }
            text
        }
        RESULT_PLANNED => "  result: planned (dry run; nothing was changed)".to_owned(),
        other => format!("  result: {other}"),
    }
}

/// The rebuild command, when this run's truncate emptied the derived index.
///
/// The fact comes from the restore's own report — whether its truncate included
/// a table the store derives — never from the backend name: a Postgres target
/// whose schema carries no derived table keeps its index, and a dry run truncates
/// nothing at all. It is deliberately independent of the result: a user-data
/// count mismatch, or a failed key swap that left the store placed and the index
/// empty, still needs the rebuild, so the notice belongs on those runs too.
fn reindex_command(report: &Report) -> Option<&'static str> {
    report.derived_index_emptied.then_some(REINDEX_COMMAND)
}

/// The schema comparison, in one line.
fn schema_text(schema: &Schema) -> String {
    format!(
        "schema: archive {} vs build {} - {}",
        schema.archive.as_deref().unwrap_or("unknown"),
        clio_store::SCHEMA_VERSION,
        schema.action
    )
}

/// What happened, or would happen, to the key store. The key store holds the
/// keys for the content the restore just placed, so whether it moved with the
/// database is the difference between readable content and unreadable content,
/// and it is stated on every run rather than only on a failure.
fn key_store_text(report: &Report) -> String {
    let Some(path) = report.target.kms_path.as_ref() else {
        return "  key store: not part of this restore".to_owned();
    };
    if !report.key_restored {
        return "  key store: not part of this restore".to_owned();
    }
    if report.dry_run {
        return format!("  key store: {} would be replaced", path.display());
    }
    if report.result == RESULT_PARTIAL && report.key_replaced == Some(false) {
        return format!(
            "  key store: {} was NOT replaced, so it still holds the old keys",
            path.display()
        );
    }
    format!("  key store: {} was replaced", path.display())
}

/// The counts, in one phrase; the bank list is named when the store holds any.
/// Each noun's singular form comes from the one shared helper, so a count of 1
/// reads correctly here and in the hygiene command that shares the phrase.
fn counts_text(counts: &Counts) -> String {
    let mut text = format!(
        "{}, {}, {}",
        count_noun(counts.items, "item", "items"),
        count_noun(counts.triples, "triple", "triples"),
        count_noun(counts.tables as u64, "table", "tables"),
    );
    if !counts.banks.is_empty() {
        text.push_str(", banks: ");
        text.push_str(counts.banks.join(", ").as_str());
    }
    text
}

/// The pre-restore line for a plan: it will be taken, or why it will not.
fn pre_restore_plan_text(pre: &PreRestore) -> String {
    match pre.reason {
        Some(reason) => format!("pre-restore snapshot: skipped ({reason})"),
        None => "a pre-restore snapshot will be taken first".to_owned(),
    }
}

/// The pre-restore line for a report: the id and path, or why it was skipped.
fn pre_restore_result_text(pre: &PreRestore) -> String {
    match (&pre.id, &pre.path) {
        (Some(id), Some(path)) => format!(
            "pre-restore snapshot: {id} ({}) - the way back",
            path.display()
        ),
        _ => format!(
            "pre-restore snapshot: skipped ({})",
            pre.reason.unwrap_or("no reason recorded")
        ),
    }
}

/// The JSON report: the fixed restore shape.
pub(super) fn payload(report: &Report) -> Value {
    let pre = &report.pre_restore;
    let reason = pre.reason.or_else(|| {
        (report.dry_run && pre.id.is_none()).then_some("dry run: no snapshot was created")
    });
    let mut payload = json!({
        // `ok` means the restore succeeded and nothing about it is flagged as
        // not durable. A count-mismatched restore and a failed key swap are both
        // failures, and so is a rename whose directory entry could not be
        // fsynced: a script reading `ok` must not record that run as clean. A
        // run whose only difference is a derived table is not ok either: it
        // exits 75, and `ok` agrees with the exit code, so `ok` is true only
        // where the code is 0.
        "ok": matches!(report.result, RESULT_RESTORED | RESULT_PLANNED) && is_durable(report),
        "snapshot_id": report.snapshot_id,
        "target": {
            "path": report.target.display,
            "backend": report.target.backend,
            "database": report.target.database,
            "schema": report.store_schema,
        },
        "pre_restore_snapshot": {
            "id": pre.id,
            "path": pre.path.as_ref().map(|path| path.display().to_string()),
            "skipped": pre.reason.is_some(),
            "reason": reason,
        },
        "counts": {
            "before": counts_json(&report.counts_before),
            "after": report.counts_after.as_ref().map(counts_json),
            "incoming": report.counts_incoming.as_ref().map(counts_json),
        },
        "schema": {
            "archive": report.schema.archive,
            "build": clio_store::SCHEMA_VERSION,
            "action": report.schema.action,
        },
        "would_replace": would_replace(report),
        "key_store": {
            "in_archive": report.key_restored,
            "replaced": report.key_replaced,
            "path": report.target.kms_path.as_ref().map(|path| path.display().to_string()),
        },
        "result": report.result,
        "failed_step": report.failed_step,
        "failure": report.failure,
        "derived_difference": report.derived_difference,
        "dry_run": report.dry_run,
        "directory_synced": report.directory_synced,
    });
    // The distinct field for the durability warning: present only when the run
    // completed but its rename is not crash-durable, so a script can tell that
    // case from both a clean restore and a partial one.
    if !is_durable(report) {
        payload["durability_warning"] = Value::String(NOT_DURABLE.to_owned());
    }
    // The distinct field for the rebuild a Postgres restore leaves behind:
    // present only when the run's own truncate emptied the derived index, which
    // is the fact the text line reads too, so a script can tell the run that
    // needs one from every other.
    if let Some(command) = reindex_command(report) {
        payload["reindex_command"] = Value::String(command.to_owned());
    }
    payload
}

/// Whether the run left nothing flagged as not durable. `None` for a run that
/// renamed no file (a dry run, or a Postgres restore), which is neither durable
/// nor flagged.
fn is_durable(report: &Report) -> bool {
    report.directory_synced != Some(false)
}

/// The files a dry run would replace: the store, its sidecars, and the key
/// store when the archive carries one. Empty for a real run, whose report
/// describes what happened rather than what would happen.
fn would_replace(report: &Report) -> Vec<String> {
    if !report.dry_run {
        return Vec::new();
    }
    if report.target.is_postgres() {
        let mut paths = vec![format!(
            "every row in postgres database {}",
            report.target.database
        )];
        if report.key_restored
            && let Some(kms) = &report.target.kms_path
        {
            paths.push(kms.display().to_string());
        }
        return paths;
    }
    let mut paths = vec![report.target.path.display().to_string()];
    if report.key_restored
        && let Some(kms) = &report.target.kms_path
    {
        paths.push(kms.display().to_string());
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut os = report.target.path.as_os_str().to_os_string();
        os.push(suffix);
        paths.push(PathBuf::from(os).display().to_string());
    }
    paths
}

/// One counts object: the two headline tables, the table count, and the banks.
fn counts_json(counts: &Counts) -> Value {
    json!({
        "items": counts.items,
        "triples": counts.triples,
        "tables": counts.tables,
        "banks": counts.banks,
    })
}

/// The rebuild line and field a placed Postgres restore carries.
#[cfg(test)]
#[path = "backup_restore_report_reindex_tests.rs"]
mod reindex_tests;
