// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio` intent, context-composition, and
//! audit read commands: `intent gate`, `compose-context`, and `audit trail`.
//!
//! ## Owns
//! - Flag/positional decoding for the gate, compose, and audit verbs, their
//!   human text views, and the defensive secret-mask pass over audit output
//! - The `compose-context` temporal-anchor flag mapping (`--query-anchor`)
//!
//! ## Does not own
//! - Gate/compose/audit semantics (see `clio-retrieve` / `clio-compliance`)
//! - The read engine and in-process call bridge (see `cli_read`)
//!
//! ## Boundary
//!
//! Changes to an intent/compose/audit read command's flags or views should
//! normally require changes here. Changes to their semantics should not.
//!
//! Keep this module focused on the intent, compose, and audit read commands.

use crate::cli_args::{FlagSpec, ParsedArgs};
use crate::cli_error::CliError;
use crate::cli_read::{one_positional, parse_u64, read_args, validate_choice, validate_time_axis};
use crate::cli_read_group::ReadGroup;
use serde_json::Value;

/// The intent, context-composition, and audit read commands.
pub(crate) struct WorkspaceGroup;

impl ReadGroup for WorkspaceGroup {
    fn verbs(&self) -> &'static [&'static str] {
        &["intent gate", "compose-context", "audit trail"]
    }

    fn spec(&self, verb: &str) -> FlagSpec {
        match verb {
            "compose-context" => FlagSpec::new(
                &[
                    "query",
                    "domains",
                    "budget-tokens",
                    "as-of",
                    "query-anchor",
                    "time-axis",
                    "recall-scope",
                ],
                &["expand-graph", "prefer-consolidated", "explain"],
            ),
            _ => FlagSpec::new(&[], &["explain"]),
        }
    }

    fn build(&self, verb: &str, parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
        match verb {
            "intent gate" => intent_gate_call(parsed),
            "compose-context" => compose_context_call(parsed),
            _ => audit_trail_call(parsed),
        }
    }

    fn render(&self, verb: &str, payload: &Value, _palette: crate::cli_style::Palette) -> String {
        match verb {
            "intent gate" => render_gate(payload),
            "compose-context" => render_compose(payload),
            _ => render_audit(payload),
        }
    }

    fn decorate(&self, verb: &str, _parsed: &ParsedArgs, _arguments: &Value, payload: &mut Value) {
        if verb == "audit trail" {
            // Defence in depth: the tool already masks, but never let a
            // secret-looking string field leave through the audit view. The
            // string-only variant keeps non-secret numeric counters (whose
            // names may merely contain a marker substring) at their own type,
            // preserving parity with the tool result.
            clio_config::secret::mask_string_secrets("", payload);
        }
    }
}

fn intent_gate_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    let turn_text = one_positional(parsed, "intent gate", "TURN_TEXT")?;
    let mut args = read_args(parsed);
    args.insert("turn_text".into(), Value::from(turn_text));
    Ok(("intent_gate", Value::Object(args)))
}

fn compose_context_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    let mut args = read_args(parsed);
    let query = parsed.value("query").or_else(|| parsed.positional(0));
    if let Some(query) = query {
        args.insert("query".into(), Value::from(query));
    }
    if parsed.positional(1).is_some() {
        return Err(crate::cli_read_args::unexpected_extra(
            parsed,
            1,
            Some("compose-context"),
            "[QUERY] --budget-tokens N",
        ));
    }
    let Some(budget) = parsed.value("budget-tokens") else {
        return Err(CliError::usage_hint(
            "`compose-context` requires a --budget-tokens value",
            "try `--budget-tokens 2000`",
        ));
    };
    args.insert(
        "budget_tokens".into(),
        Value::from(parse_u64("budget-tokens", budget)?),
    );
    if let Some(domains) = parsed.value("domains") {
        args.insert("domains".into(), Value::from(split_domains(domains)));
    }
    if let Some(as_of) = parsed.value("as-of") {
        args.insert("as_of".into(), Value::from(as_of));
    }
    if let Some(anchor) = parsed.value("query-anchor") {
        args.insert("query_anchor".into(), Value::from(anchor));
    }
    if let Some(axis) = parsed.value("time-axis") {
        args.insert("time_axis".into(), Value::from(validate_time_axis(axis)?));
    }
    if let Some(scope) = parsed.value("recall-scope") {
        args.insert(
            "recall_scope".into(),
            Value::from(validate_choice(
                "recall-scope",
                scope,
                &["full", "consolidated_only"],
            )?),
        );
    }
    if parsed.has("expand-graph") {
        args.insert("expand_graph".into(), Value::Bool(true));
    }
    if parsed.has("prefer-consolidated") {
        args.insert("prefer_consolidated".into(), Value::Bool(true));
    }
    Ok(("compose_context", Value::Object(args)))
}

fn audit_trail_call(parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    let item_id = one_positional(parsed, "audit trail", "ITEM_ID")?;
    let mut args = read_args(parsed);
    args.insert("item_id".into(), Value::from(item_id));
    Ok(("audit_trail", Value::Object(args)))
}

fn split_domains(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|domain| !domain.is_empty())
        .map(str::to_owned)
        .collect()
}

fn render_gate(payload: &Value) -> String {
    let needed = payload
        .get("retrieval_needed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut lines = vec![format!("retrieval_needed: {needed}")];
    if let Some(domains) = payload.get("domains").and_then(Value::as_array) {
        let domains: Vec<&str> = domains.iter().filter_map(Value::as_str).collect();
        lines.push(format!("domains: {}", domains.join(",")));
    }
    if let Some(confidence) = payload.get("confidence").and_then(Value::as_f64) {
        lines.push(format!("confidence: {confidence}"));
    }
    if let Some(reason) = payload.get("reason").and_then(Value::as_str) {
        lines.push(format!("reason: {reason}"));
    }
    lines.join("\n")
}

fn render_compose(payload: &Value) -> String {
    let number = |key: &str| {
        payload
            .get(key)
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .to_string()
    };
    let sections = payload
        .get("sections")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let truncated = payload
        .get("truncated_ids")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    format!(
        "sections: {sections}\ntotal_tokens: {}\nbudget_tokens: {}\ntruncated_ids: {truncated}",
        number("total_tokens"),
        number("budget_tokens")
    )
}

fn render_audit(payload: &Value) -> String {
    let mut lines = vec![format!(
        "item_id: {}",
        payload
            .get("item_id")
            .and_then(Value::as_str)
            .unwrap_or("?")
    )];
    for key in ["lineage_ids", "revisions", "events", "timeline"] {
        let count = payload
            .get(key)
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        lines.push(format!("{key}: {count}"));
    }
    lines.join("\n")
}

#[cfg(test)]
#[path = "cli_read_workspace_tests.rs"]
mod tests;
