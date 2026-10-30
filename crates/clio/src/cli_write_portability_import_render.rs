// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio import` human text view: the
//! summary line (created/skipped/rejected/overwritten plus the skipped-invalid
//! naming tail) and the detail lines for the fields the JSON report carries —
//! `written`, `created_ids`, `rejection_samples`, and `notes`.
//!
//! ## Owns
//! - The `import` text view over the MCP `import` payload
//! - The skipped-as-invalid tail and the per-run detail lines
//!
//! ## Does not own
//! - Import behaviour and report shape (see `clio-compliance`)
//! - The write engine and portability command decoding (see
//!   `cli_write_portability`)
//! - The `export` and `erase` text views
//!
//! ## Boundary
//!
//! Changes to the `import` human text view should normally require changes
//! here. Changes to the report shape should not require changes here.
//!
//! Keep this module focused on the `import` text view.

use serde_json::Value;

/// Render the `import` payload as deterministic human text.
///
/// The summary line comes first; then one line per detail field the payload
/// carries (`written`, `created_ids`, `rejection_samples`, `notes`), so the
/// text form carries the same fields as the JSON form.
pub(crate) fn render_import(payload: &Value) -> String {
    let report = payload.get("report").unwrap_or(&Value::Null);
    let count = |key: &str| report.get(key).and_then(Value::as_u64).unwrap_or(0);
    let (create, skip, reject, overwrite) = (
        count("would_create"),
        count("would_skip"),
        count("would_reject"),
        count("would_overwrite"),
    );
    let summary = if report.get("dry_run") == Some(&Value::Bool(true)) {
        format!("import dry-run: would create {create}, skip {skip}, reject {reject} (zero writes)")
    } else if report.get("applied") != Some(&Value::Bool(true)) {
        format!(
            "import applied nothing: would create {create}, skip {skip}, \
             reject {reject}, overwrite {overwrite}"
        )
    } else {
        let invalid = skipped_invalid_tail(report);
        // A live run that created and overwrote nothing restored nothing,
        // however many rows it walked; "created 0" alone reads like a
        // successful restore.
        if create == 0 && overwrite == 0 {
            format!(
                "import: nothing restored: created 0, overwrote 0, skipped {skip}, \
                 rejected {reject}{invalid}"
            )
        } else {
            format!(
                "import: created {create}, skipped {skip}, rejected {reject}, \
                 overwrote {overwrite}{invalid}"
            )
        }
    };
    let mut lines = vec![summary];
    lines.extend(detail_lines(payload, report));
    lines.join("\n")
}

/// One line per detail field the payload carries: `written` from the payload,
/// then `dry_run`, `applied`, `created_ids`, `rejection_samples`, and `notes`
/// from the report. A field the payload does not carry adds no line; a
/// carried-but-empty field is named `none` rather than dropped.
fn detail_lines(payload: &Value, report: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(written) = payload.get("written").and_then(Value::as_array) {
        let restored = written
            .iter()
            .map(|row| {
                format!(
                    "{}/{}",
                    row.get("bank").and_then(Value::as_str).unwrap_or("?"),
                    row.get("id").and_then(Value::as_str).unwrap_or("?")
                )
            })
            .collect::<Vec<_>>();
        lines.push(format!("written: {}", named(&restored)));
    }
    for key in ["dry_run", "applied"] {
        if let Some(value) = report.get(key) {
            lines.push(format!("{key}: {}", entry_text(value)));
        }
    }
    for key in ["created_ids", "rejection_samples", "notes"] {
        if let Some(entries) = report.get(key).and_then(Value::as_array) {
            let rendered = entries.iter().map(entry_text).collect::<Vec<_>>();
            lines.push(format!("{key}: {}", named(&rendered)));
        }
    }
    lines
}

/// The text for one entry: strings bare, other values as their JSON text.
fn entry_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// A list rendered on one line, or `none` when it is empty.
fn named(entries: &[String]) -> String {
    if entries.is_empty() {
        "none".to_owned()
    } else {
        entries.join(", ")
    }
}

/// Entities named per line before the text view falls back to a count.
const INVALID_NAMED: usize = 3;

/// The text-mode tail naming the entities an import skipped as invalid: the
/// count, then up to `INVALID_NAMED` ids with their code and masked reason, then
/// how many the report holds back. Empty when nothing was skipped as invalid, so
/// the JSON report stays the authoritative record and the text only adds to it.
fn skipped_invalid_tail(report: &Value) -> String {
    let field = |row: &Value, key: &str| {
        row.get(key)
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_owned()
    };
    let total = report
        .get("would_skip_invalid")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let named: Vec<String> = report
        .get("skipped_invalid")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .take(INVALID_NAMED)
        .map(|row| {
            format!(
                "{} ({}: {})",
                field(row, "id"),
                field(row, "code"),
                field(row, "reason")
            )
        })
        .collect();
    if total == 0 && named.is_empty() {
        return String::new();
    }
    let hidden = total.saturating_sub(named.len() as u64);
    let more = match hidden {
        0 => String::new(),
        _ => format!(" and {hidden} more"),
    };
    if named.is_empty() {
        return format!(", skipped invalid {total}{more}");
    }
    format!(", skipped invalid {total}: {}{more}", named.join("; "))
}

#[cfg(test)]
#[path = "cli_write_portability_import_render_tests.rs"]
mod import_render_tests;
