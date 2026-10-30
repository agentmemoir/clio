// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio ops` output layer: the human text
//! renderings of the `verify` and `diagnose` reports, the mode-resolved report
//! emitter, and the shared ops error/JSON emitters the dispatch calls.
//!
//! ## Owns
//! - `render_verify` and `render_diagnose`: the human text for the two read-only
//!   health reports
//! - `emit_report`: one emission point that picks JSON or text from `OutputMode`
//! - `print_json`, `emit_error`, `emit_usage_error`, and `redact`, the emitters
//!   every ops subcommand and error path shares
//!
//! ## Does not own
//! - The `clio ops` dispatch, flag handling, and exit codes (`ops_cli`)
//! - The report shapes and check semantics (`clio-ops`)
//! - The shared output-mode contract (`cli_output`)
//!
//! ## Boundary
//!
//! Changes to how an ops report is presented should normally require changes
//! here. Changes to what verify or diagnose check should not require changes
//! here.
//!
//! Every JSON field reappears in the text rendering: a human reading text mode
//! must never see less than a reader of the JSON line.

use clio_ops::{DiagnoseReport, OpsFinding, VerifyReport};
use std::io::Write;
use std::process::ExitCode;

use crate::cli_output::{self, OutputMode};
use clio_types::AmError;

/// Emit a report in the resolved mode: one compact JSON line, or the human text
/// block the caller rendered from the same report.
pub(crate) fn emit_report<S: serde::Serialize>(
    report: &S,
    text: &str,
    mode: OutputMode,
    mut out: &mut dyn Write,
) {
    let _ = match mode {
        // The report serializes itself here rather than through
        // `serde_json::Value`: a `Value` object sorts keys, and the JSON line
        // keeps the report's own field order, so the piped default stays
        // byte-identical to the pre-mode emission.
        OutputMode::Json => {
            let line = serde_json::to_string(report).unwrap_or_else(|_| "{}".to_owned());
            writeln!(out, "{line}")
        }
        OutputMode::Text => cli_output::write_text(&mut out, text),
    };
}

/// Print a serializable value as one JSON line (the unchanged path for the
/// subcommands that keep JSON-only output).
pub(crate) fn print_json(value: &impl serde::Serialize) {
    let line = serde_json::to_string(value).unwrap_or_else(|_| "{}".into());
    println!("{line}");
}

/// Render an error as structured JSON and return the mapped exit code.
pub(crate) fn emit_error(error: &AmError, code: u8) -> ExitCode {
    let exit_code = if error.code == clio_types::ErrorCode::StorageBusy {
        crate::cli_error::CliError::from_am_error(error).exit_code()
    } else {
        ExitCode::from(code)
    };
    print_json(&serde_json::json!({
        "ok": false,
        "code": error.code.as_str(),
        "message": redact(&error.message),
    }));
    exit_code
}

/// Render a bad flag value with the shared usage code and exit group.
pub(crate) fn emit_usage_error(message: impl AsRef<str>) -> ExitCode {
    print_json(&serde_json::json!({
        "ok": false,
        "code": "usage",
        "message": redact(message.as_ref()),
    }));
    ExitCode::from(2)
}

/// Strip credentials from any URL embedded in a driver error message (shared
/// redactor, also applied by the MCP ops surfaces).
pub(crate) fn redact(message: &str) -> String {
    clio_ops::redact_credentials(message)
}

/// Report a usage failure the way every other command surface reports one: to
/// stderr, under the mode the raw tokens imply, with the shared usage exit code.
pub(crate) fn report_output_error(
    error: &crate::cli_error::CliError,
    raw: &[String],
    is_tty: bool,
) -> ExitCode {
    let fallback = cli_output::mode_from_raw(raw, is_tty);
    cli_output::report_error(error, fallback, is_tty, cli_output::no_color_requested())
}

/// The human rendering of a verify report: the verdict, scope, timestamp,
/// coverage counts, integrity signal counts, findings, and notes.
pub(crate) fn render_verify(report: &VerifyReport) -> String {
    let mut lines = vec![
        format!("verify: {}", verdict(report.ok)),
        format!("scope: {}", report.scope),
        format!("timestamp: {}", report.timestamp),
        format!("indexable_items: {}", report.indexable_items),
        format!("dense_rows: {}", report.dense_rows),
        format!("lexical_rows: {}", report.lexical_rows),
        "integrity signals:".to_owned(),
        format!(
            "  orphan_assoc_edges: {}",
            report.integrity.orphan_assoc_edges
        ),
        format!(
            "  orphan_triple_items: {}",
            report.integrity.orphan_triple_items
        ),
        format!(
            "  dangling_snapshot_refs: {}",
            report.integrity.dangling_snapshot_refs
        ),
        format!(
            "  dangling_gist_refs: {}",
            report.integrity.dangling_gist_refs
        ),
        format!(
            "  orphan_memtree_leaf_items: {}",
            report.integrity.orphan_memtree_leaf_items
        ),
    ];
    push_findings(&mut lines, &report.findings);
    push_notes(&mut lines, &report.notes);
    lines.join("\n")
}

/// The human rendering of a diagnose report: the verdict, timestamp, backend
/// and schema state, bank list, embedding state, coverage and job counts, the
/// deeper-check pointer, and findings.
pub(crate) fn render_diagnose(report: &DiagnoseReport) -> String {
    let banks = if report.banks.is_empty() {
        "none".to_owned()
    } else {
        report.banks.join(", ")
    };
    let mut lines = vec![
        format!("diagnose: {}", verdict(report.ok)),
        format!("timestamp: {}", report.timestamp),
        format!(
            "backend: {}",
            if report.backend_reachable {
                "reachable"
            } else {
                "unreachable"
            }
        ),
        format!(
            "schema_version: {}",
            or_none(report.schema_version.as_deref())
        ),
        format!(
            "expected_schema_version: {}",
            report.expected_schema_version
        ),
        format!("banks: {banks}"),
        format!(
            "vector_index: {}",
            if report.vector_index_present {
                "present"
            } else {
                "absent"
            }
        ),
        format!(
            "embedding_model: {}",
            or_none(report.embedding_model.as_deref())
        ),
        format!(
            "embedding_dims: {}",
            report
                .embedding_dims
                .map_or_else(|| "none".to_owned(), |dims| dims.to_string())
        ),
        format!("index_mode: {}", or_none(report.index_mode.as_deref())),
        format!("indexable_items: {}", report.indexable_items),
        format!("dense_rows: {}", report.dense_rows),
        format!("lexical_rows: {}", report.lexical_rows),
        format!("pending_jobs: {}", report.pending_jobs),
        format!("retry_jobs: {}", report.retry_jobs),
        format!(
            "embedding_ready: {}",
            match report.embedding_ready {
                Some(true) => "yes",
                Some(false) => "no",
                None => "not probed",
            }
        ),
        format!("deeper checks: {}", report.verify_pointer),
    ];
    push_findings(&mut lines, &report.findings);
    lines.join("\n")
}

/// The one-word verdict a report opens with.
fn verdict(ok: bool) -> &'static str {
    if ok { "ok" } else { "failed" }
}

/// `Some(value)` rendered as itself, `None` as `none`.
fn or_none(value: Option<&str>) -> &str {
    value.unwrap_or("none")
}

/// Append the findings block: a count header, then one line per finding.
fn push_findings(lines: &mut Vec<String>, findings: &[OpsFinding]) {
    if findings.is_empty() {
        lines.push("findings: none".to_owned());
        return;
    }
    lines.push(format!("findings: {}", findings.len()));
    for finding in findings {
        lines.push(format!(
            "  {} {} {}: {}",
            severity_word(finding.severity),
            finding.code,
            finding.scope,
            finding.message
        ));
    }
}

/// Append each note on its own line, or a bare `notes: none`.
fn push_notes(lines: &mut Vec<String>, notes: &[String]) {
    if notes.is_empty() {
        lines.push("notes: none".to_owned());
        return;
    }
    for note in notes {
        lines.push(format!("note: {note}"));
    }
}

/// The lowercase severity word, as the JSON spells it.
fn severity_word(severity: clio_ops::Severity) -> &'static str {
    match severity {
        clio_ops::Severity::Info => "info",
        clio_ops::Severity::Warning => "warning",
        clio_ops::Severity::Error => "error",
    }
}

#[cfg(test)]
#[path = "ops_cli_render_tests.rs"]
mod tests;
