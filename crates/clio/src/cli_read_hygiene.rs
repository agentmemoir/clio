// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio` hygiene read commands
//! `hygiene audit` and `hygiene log`: flag decoding into their read-only MCP
//! tools, the audit page's in-band `truncated` marker, and their human text
//! views.
//!
//! ## Owns
//! - Flag decoding for `hygiene audit` / `hygiene log` into MCP tool arguments,
//!   under the shared `--limit` page rule
//! - The audit page's bounded-output `truncated` and scan-budget `incomplete`
//!   markers
//! - The audit and log pages' `--limit` clamp disclosure in both output forms
//! - Their human text views (the audit text view never prints previews)
//!
//! ## Does not own
//! - Scoring, scanning, or clean semantics (see `clio-hygiene` / `clio-mcp`)
//! - The read engine and in-process call bridge (see `cli_read`)
//!
//! ## Boundary
//!
//! Changes to the audit/log flags or views should normally require changes
//! here. Changes to hygiene scoring or storage should not require changes here.
//!
//! Keep this module focused on the hygiene read commands.

use crate::cli_args::{FlagSpec, ParsedArgs};
use crate::cli_error::CliError;
use crate::cli_read::{
    MAX_PAGE_LIMIT, add_requested_limit, base_args, no_positionals, page_limit, parse_f64,
    parse_u64,
};
use crate::cli_read_group::ReadGroup;
use crate::count_noun::count_noun;
use serde_json::Value;

/// The hygiene read commands.
pub(crate) struct HygieneReadGroup;

impl ReadGroup for HygieneReadGroup {
    fn verbs(&self) -> &'static [&'static str] {
        &["hygiene audit", "hygiene log"]
    }

    // The registry record owns each verb's accepted set, so the parser and the
    // usage line cannot disagree; a name with no record accepts only the globals.
    fn spec(&self, verb: &str) -> FlagSpec {
        match crate::cli_command_registry::find(verb) {
            Some(record) => record.parser_spec(),
            None => FlagSpec::new(&[], &[]),
        }
    }

    fn build(&self, verb: &str, parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
        match verb {
            "hygiene audit" => audit_call(parsed),
            _ => log_call(parsed),
        }
    }

    fn decorate(&self, verb: &str, parsed: &ParsedArgs, _arguments: &Value, payload: &mut Value) {
        if verb != "hygiene audit" {
            // The log page has no truncation marker of its own; only a lowered
            // page request needs naming.
            add_requested_limit(payload, parsed);
            return;
        }
        // Bounded-output contract: mark the page truncated when more ranked
        // candidates exist beyond `offset` + the returned page.
        let returned = payload
            .get("candidates")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let offset = parsed
            .value("offset")
            .and_then(|raw| raw.parse::<usize>().ok())
            .unwrap_or(0);
        let total = payload
            .get("total_candidates")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0);
        payload["truncated"] = Value::Bool(offset.saturating_add(returned) < total);
        // The CLI clamped a typed `--limit` above the cap; name the value the
        // caller asked for so the served page never silently differs.
        add_requested_limit(payload, parsed);
    }

    fn render(&self, verb: &str, payload: &Value, _palette: crate::cli_style::Palette) -> String {
        match verb {
            "hygiene audit" => render_audit(payload),
            _ => render_log(payload),
        }
    }
}

/// `hygiene audit [--min-score N] [--limit N] [--offset N]` → `hygiene_audit`:
/// the ranked read-only noise scan over one bank.
fn audit_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    no_positionals(parsed)?;
    let mut args = base_args(parsed);
    if let Some(min_score) = parsed.value("min-score") {
        args.insert(
            "min_score".into(),
            Value::from(parse_f64("min-score", min_score)?),
        );
    }
    if let Some(limit) = parsed.value("limit") {
        // One shared page-size rule: `0` refused, above the cap clamped (the
        // tool's own validator stays as the backstop for direct callers).
        let served = page_limit("limit", Some(parse_u64("limit", limit)?))?;
        args.insert("limit".into(), Value::from(served));
    }
    if let Some(offset) = parsed.value("offset") {
        args.insert("offset".into(), Value::from(parse_u64("offset", offset)?));
    }
    Ok(("hygiene_audit", Value::Object(args)))
}

/// `hygiene log [--limit N] [--offset N]` → `hygiene_log_list`: the masked
/// durable hygiene log rows for one bank.
fn log_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    no_positionals(parsed)?;
    let mut args = base_args(parsed);
    if let Some(limit) = parsed.value("limit") {
        // One shared page-size rule: `0` refused, above the cap clamped (the
        // store's own 1..=200 validator stays as the backstop for direct
        // callers). Omitted keeps the tool's default of 50.
        let served = page_limit("limit", Some(parse_u64("limit", limit)?))?;
        args.insert("limit".into(), Value::from(served));
    }
    if let Some(offset) = parsed.value("offset") {
        args.insert("offset".into(), Value::from(parse_u64("offset", offset)?));
    }
    Ok(("hygiene_log_list", Value::Object(args)))
}

/// Human text for the audit report. One summary line, then one line per
/// candidate with score, suggested action, id, and reasons — never the preview
/// field (defense in depth; the tool already masks it).
fn render_audit(payload: &Value) -> String {
    let bank = payload.get("bank").and_then(Value::as_str).unwrap_or("?");
    let scanned = payload.get("scanned").and_then(Value::as_u64).unwrap_or(0);
    let total = payload
        .get("total_candidates")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut summary = format!(
        "hygiene audit (bank {bank}): scanned {}, {}",
        count_noun(scanned, "active item", "active items"),
        count_noun(total, "candidate", "candidates"),
    );
    // Bounded-output contract: never silently truncate the scan. The tool marks
    // a bank larger than the scan budget via `incomplete`; surface it in text.
    if payload.get("incomplete") == Some(&Value::Bool(true)) {
        summary.push_str(" (partial scan: bank exceeds the scan budget)");
    }
    // The CLI clamped the typed `--limit` down to the cap; name both values so
    // the page never silently disagrees with the request.
    if let Some(requested) = payload.get("requested_limit").and_then(Value::as_u64) {
        use std::fmt::Write as _;
        let _ = write!(
            summary,
            " (limit {MAX_PAGE_LIMIT}, lowered from {requested})"
        );
    }
    // Bounded-output contract: a text page must not silently drop rows. The
    // payload does not echo the request offset, so the note names the flag.
    if payload.get("truncated") == Some(&Value::Bool(true)) {
        use std::fmt::Write as _;
        let printed = payload
            .get("candidates")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let _ = write!(
            summary,
            ", printed {} (truncated: use --offset for the next page)",
            count_noun(printed as u64, "row", "rows"),
        );
    }
    let mut lines = vec![summary];
    if let Some(candidates) = payload.get("candidates").and_then(Value::as_array) {
        for candidate in candidates {
            let score = candidate
                .get("noise_score")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let action = candidate
                .get("suggested_action")
                .and_then(Value::as_str)
                .unwrap_or("-");
            let item_id = candidate
                .get("item_id")
                .and_then(Value::as_str)
                .unwrap_or("?");
            let reasons = candidate
                .get("noise_reasons")
                .and_then(Value::as_array)
                .map_or_else(String::new, |reasons| {
                    reasons
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(",")
                });
            lines.push(format!("  {score:.2}  {action}  {item_id}  {reasons}"));
        }
    }
    lines.join("\n")
}

/// Human text for the log rows: one count line, then timestamp, bank, action,
/// state, and item id per row (rows carry ids and labels only by construction).
/// The state is the row's `detail.result`, so a previewed row and an applied one
/// cannot read alike; a row that recorded none reads `unknown`.
fn render_log(payload: &Value) -> String {
    let rows = payload.get("rows").and_then(Value::as_array);
    let count = rows.map_or(0, Vec::len);
    // The count chooses its own noun through the one shared helper, so a
    // one-row log reads "1 entry" while an empty one keeps the plural.
    let mut summary = format!(
        "hygiene log: {}",
        count_noun(count as u64, "entry", "entries")
    );
    // The CLI clamped the typed `--limit` down to the cap; the log page does
    // not echo its page size, so name the cap and the request together.
    if let Some(requested) = payload.get("requested_limit").and_then(Value::as_u64) {
        use std::fmt::Write as _;
        let _ = write!(
            summary,
            " (limit {MAX_PAGE_LIMIT}, lowered from {requested})"
        );
    }
    let mut lines = vec![summary];
    if let Some(rows) = rows {
        for row in rows {
            let timestamp = row.get("timestamp").and_then(Value::as_str).unwrap_or("?");
            let bank = row.get("bank").and_then(Value::as_str).unwrap_or("?");
            let action = row.get("action").and_then(Value::as_str).unwrap_or("?");
            let result = row
                .get("detail")
                .and_then(|detail| detail.get("result"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let item_id = row.get("item_id").and_then(Value::as_str).unwrap_or("?");
            lines.push(format!(
                "{timestamp}  {bank}  {action}  {result}  {item_id}"
            ));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
#[path = "cli_read_hygiene_tests.rs"]
mod tests;

// The truncated-page text tests are a child module so both files keep real
// headroom under the 450-line source-file limit.
#[cfg(test)]
#[path = "cli_read_hygiene_tests_truncation_tests.rs"]
mod truncation_tests;

// The page-limit clamp tests are a child module so both files keep real
// headroom under the 450-line source-file limit.
#[cfg(test)]
#[path = "cli_read_hygiene_tests_limit_tests.rs"]
mod limit_tests;
