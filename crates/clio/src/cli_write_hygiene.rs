// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio hygiene clean` command: reading
//! prior audit candidates from a JSON file (or stdin via `--file -`) and
//! handing them to the `hygiene_clean` tool, where an unconfirmed call (or an
//! explicit `--dry-run`) is a zero-write dry-run and `--confirm` applies the
//! action. A confirmed batch abort is surfaced in the text view and exits 1.
//!
//! ## Owns
//! - The candidates file/stdin decoding (fail-closed before any dispatcher call)
//! - The `hygiene clean` flag decoding and human text view
//!
//! ## Does not own
//! - The write engine, parsing, and in-process call bridge (see `cli_write`)
//! - Clean semantics, per-item gating, or the dry-run decision (see
//!   `clio-hygiene` / `clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to the candidates-file format or clean flags should normally require
//! changes here. Changes to clean semantics should not require changes here.
//!
//! Keep this module focused on the `hygiene clean` command.

use crate::cli_args::{FlagSpec, ParsedArgs};
use crate::cli_error::{CliError, EXIT_OPERATIONAL};
use crate::cli_read::{CliIo, base_args, validate_choice};
use crate::cli_write_group::{Refusal, WriteCall, WriteGroup};
use serde_json::Value;
use std::fmt::Write as _;

/// The confirmed-or-dry-run hygiene cleanup command.
pub(crate) struct HygieneWriteGroup;

/// The closed action set; `delete` is the documented non-compliance alias of
/// `discard` (it never touches DEKs or tombstones).
const ACTIONS: &[&str] = &["flag", "archive", "discard", "delete", "keep"];

impl WriteGroup for HygieneWriteGroup {
    fn verbs(&self) -> &'static [&'static str] {
        &["hygiene clean"]
    }

    fn spec(&self, _verb: &str) -> FlagSpec {
        FlagSpec::new(
            &["file", "action", "reason", "note"],
            &["confirm", "dry-run"],
        )
    }

    fn build(&self, _verb: &str, parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
        // No injected streams on this path: `--file -` cannot read stdin here.
        clean_call(parsed, None)
    }

    fn build_with_io(
        &self,
        _verb: &str,
        parsed: &ParsedArgs,
        io: &mut CliIo<'_>,
    ) -> Result<WriteCall, CliError> {
        clean_call(parsed, Some(&mut *io.stdin))
    }

    fn render(&self, _verb: &str, payload: &Value, _palette: crate::cli_style::Palette) -> String {
        render_clean(payload)
    }

    fn mutates(&self, _verb: &str) -> bool {
        true
    }

    fn refusal(&self, _verb: &str, _payload: &Value) -> Option<Refusal> {
        // Hygiene payloads carry no admission decision; nothing to refuse here.
        None
    }

    fn failure_exit(&self, _verb: &str, payload: &Value) -> Option<u8> {
        // A confirmed batch abort is a partial failure: earlier targets may have
        // applied, the failing target is reported in `failed`, and the rest are
        // `pending`. Keep the payload but exit 1 so a script cannot mistake it
        // for a clean success. (`failed` serializes as `null` on success.)
        payload
            .get("failed")
            .is_some_and(|failed| !failed.is_null())
            .then_some(EXIT_OPERATIONAL)
    }
}

/// `hygiene clean --file candidates.json [--action A] [--confirm] [--dry-run] ...`:
/// decode the candidates file (or stdin via `--file -`) fail-closed, then hand
/// them to the `hygiene_clean` tool.
///
/// Without `--confirm` (or the global `--yes`), or with `--dry-run`, the call
/// still goes to the tool WITHOUT confirm: the tool dry-runs (counts only, zero
/// writes) and the CLI exits 0. This is deliberately not a refusal — a dry-run
/// is not destructive.
fn clean_call(
    parsed: &ParsedArgs,
    stdin: Option<&mut dyn std::io::Read>,
) -> Result<WriteCall, CliError> {
    let path = parsed.value("file").ok_or_else(|| {
        CliError::usage_hint(
            "`hygiene clean` requires --file (a candidates JSON file, or `-` for stdin)",
            "try 'clio hygiene clean --file candidates.json'",
        )
    })?;
    let raw = match path {
        "-" => {
            let Some(stdin) = stdin else {
                return Err(CliError::usage_hint(
                    "`hygiene clean --file -` reads the candidates JSON from stdin",
                    "try 'clio hygiene clean --file - < candidates.json'",
                ));
            };
            std::io::read_to_string(stdin).map_err(|error| {
                CliError::usage(format!("`hygiene clean --file -` is not readable: {error}"))
            })?
        }
        other => std::fs::read_to_string(other).map_err(|error| {
            CliError::usage(format!("--file `{other}` is not readable: {error}"))
        })?,
    };
    let candidates = parse_candidates(&raw)?;
    let action = match parsed.value("action") {
        Some(raw) => validate_choice("action", raw, ACTIONS)?,
        None => "keep",
    };
    let mut args = base_args(parsed);
    args.insert("candidates".into(), Value::Array(candidates));
    args.insert("action".into(), Value::from(action));
    // `--dry-run` is the explicit alias for the unconfirmed preview: it forces
    // zero writes even when `--confirm`/`--yes` is also present.
    if crate::cli_confirm::confirmed(parsed) && !parsed.has("dry-run") {
        args.insert("confirm".into(), Value::Bool(true));
    }
    if let Some(reason) = parsed.value("reason") {
        args.insert("reason".into(), Value::from(reason));
    }
    if let Some(note) = parsed.value("note") {
        args.insert("note".into(), Value::from(note));
    }
    Ok(WriteCall::Tool {
        tool: "hygiene_clean",
        args: Value::Object(args),
    })
}

/// Decode the candidates document: a bare JSON array of candidate objects, or
/// a JSON object with a `candidates` array (the `hygiene_audit` report shape;
/// other report fields are ignored). Anything else fails closed with a usage
/// error before any dispatcher call.
///
/// Every candidate must be an object with a string `item_id`; an optional
/// `noise_score` (number) and `suggested_action` (string) pass through.
fn parse_candidates(raw: &str) -> Result<Vec<Value>, CliError> {
    let value: Value = serde_json::from_str(raw).map_err(|error| {
        CliError::usage(format!("--file must contain JSON candidates: {error}"))
    })?;
    let entries = match value {
        Value::Array(entries) => entries,
        Value::Object(fields) => match fields.get("candidates") {
            Some(Value::Array(entries)) => entries.clone(),
            Some(_) => {
                return Err(CliError::usage(
                    "--file `candidates` must be a JSON array of candidate objects",
                ));
            }
            None => {
                return Err(CliError::usage_hint(
                    "--file must contain a JSON object with a `candidates` array or a bare JSON array",
                    "try '{\"candidates\": [{\"item_id\": \"itm-1\"}]}'",
                ));
            }
        },
        _ => {
            return Err(CliError::usage(
                "--file must contain a JSON object with a `candidates` array or a bare JSON array",
            ));
        }
    };
    let mut candidates = Vec::with_capacity(entries.len());
    for entry in &entries {
        candidates.push(candidate(entry)?);
    }
    Ok(candidates)
}

/// Validate one candidate: object with a string `item_id`, optional numeric
/// `noise_score`, optional string `suggested_action`. Unknown extra fields
/// (e.g. the audit report's reasons/preview) are dropped; the tool owns what a
/// clean target accepts.
fn candidate(entry: &Value) -> Result<Value, CliError> {
    let Some(object) = entry.as_object() else {
        return Err(CliError::usage(
            "--file candidates entries must be JSON objects with a string `item_id`",
        ));
    };
    let item_id = object
        .get("item_id")
        .and_then(Value::as_str)
        .ok_or_else(|| CliError::usage("--file candidates entries require a string `item_id`"))?;
    let mut candidate = serde_json::Map::new();
    candidate.insert("item_id".into(), Value::from(item_id));
    if let Some(score) = object.get("noise_score") {
        if !score.is_number() {
            return Err(CliError::usage(
                "--file candidates `noise_score` must be a number",
            ));
        }
        candidate.insert("noise_score".into(), score.clone());
    }
    if let Some(action) = object.get("suggested_action") {
        let Some(action) = action.as_str() else {
            return Err(CliError::usage(
                "--file candidates `suggested_action` must be a string",
            ));
        };
        candidate.insert("suggested_action".into(), Value::from(action));
    }
    Ok(Value::Object(candidate))
}

/// Human text for the clean outcome: a dry-run counts line, or the applied
/// summary plus any per-target failure and pending count on a confirmed batch
/// abort (a failure is never hidden).
fn render_clean(payload: &Value) -> String {
    if payload.get("dry_run") == Some(&Value::Bool(true)) {
        let count = |key: &str| payload.get(key).and_then(Value::as_u64).unwrap_or(0);
        let (flag, archive, discard) = (
            count("would_flag"),
            count("would_archive"),
            count("would_discard"),
        );
        return format!(
            "hygiene clean dry-run: would flag {flag}, archive {archive}, discard {discard} \
             (zero writes; pass --confirm to apply)"
        );
    }
    let applied = count_of(payload, "applied");
    let pending = count_of(payload, "pending");
    let mut summary = format!("hygiene clean: applied {applied} item(s)");
    if let Some(failed) = payload.get("failed").filter(|failed| !failed.is_null()) {
        let item_id = failed.get("item_id").and_then(Value::as_str).unwrap_or("?");
        let error = failed
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        let _ = write!(summary, "; failed {item_id} ({error})");
    }
    if pending > 0 {
        let _ = write!(summary, "; aborted, {pending} pending");
    }
    summary
}

/// Count an array-valued (or absent) payload field.
fn count_of(payload: &Value, key: &str) -> usize {
    payload
        .get(key)
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

#[cfg(test)]
#[path = "cli_write_hygiene_tests.rs"]
mod tests;
