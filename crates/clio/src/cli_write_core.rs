// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the core `clio` write commands: `remember`
//! (flagship gated store), `admit` (read-only admission preview, including the
//! `--file` batch preview), `summarize`, and `consolidate`, with `--dry-run`
//! routing to the no-write preview primitives.
//!
//! ## Owns
//! - Flag decoding for the core write verbs into MCP tool arguments
//! - The shared decoding/validation of the optional bounded `--context` value
//!   (reused by the mutation commands' `correct`)
//! - The `--dry-run` remap (`remember` → `admit_preview`; validated previews
//!   for commands whose in-process tool has no no-write path)
//! - Their human text views
//!
//! ## Does not own
//! - The write engine, parsing, and in-process call bridge (see `cli_write`)
//! - The MCP tool semantics or gating (see `clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to a core write command's flags or arguments should normally require
//! changes here. Changes to the write engine should not require changes here.
//!
//! Keep this module focused on the core write commands.

use crate::cli_args::{FlagSpec, ParsedArgs};
use crate::cli_error::CliError;
use crate::cli_read::{base_args, no_positionals, one_positional, parse_f64, validate_choice};
use crate::cli_write_duplicate::{allow_duplicate_arg, remember_refusal};
use crate::cli_write_group::{Refusal, WriteCall, WriteGroup, batch_preview_refusal};
use crate::cli_write_source_type::source_type_arg;
use clio_types::{CONTEXT_MAX_BYTES, ErrorCode, SemanticCategory, validate_context};
use serde_json::{Map, Value, json};

/// The core write commands (`remember`, `admit`, `summarize`, `consolidate`).
pub(crate) struct CoreWriteGroup;

impl WriteGroup for CoreWriteGroup {
    fn verbs(&self) -> &'static [&'static str] {
        &["remember", "admit", "summarize", "consolidate"]
    }

    fn spec(&self, verb: &str) -> FlagSpec {
        match verb {
            "remember" => FlagSpec::new(
                &[
                    "category",
                    "epistemic-kind",
                    "confidence",
                    "source-type",
                    "evidence-ref",
                    "context",
                    "source-text",
                ],
                &["dry-run", "allow-duplicate"],
            ),
            "admit" => FlagSpec::new(
                &[
                    "category",
                    "epistemic-kind",
                    "confidence",
                    "source-type",
                    "evidence-ref",
                    "context",
                    "file",
                ],
                &[],
            ),
            "summarize" => FlagSpec::new(&["scope"], &["dry-run"]),
            _ => FlagSpec::new(&[], &["force", "dry-run"]),
        }
    }

    fn build(&self, verb: &str, parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
        match verb {
            "remember" => remember_call(parsed),
            "admit" => admit_call(parsed),
            "summarize" => summarize_call(parsed),
            _ => consolidate_call(parsed),
        }
    }

    fn render(&self, verb: &str, payload: &Value, _palette: crate::cli_style::Palette) -> String {
        match verb {
            // `remember --dry-run` remaps to `admit_preview`: the payload is a
            // read-only decision (no `id`), so it must not render as a write.
            "remember" if payload.get("dry_run") == Some(&Value::Bool(true)) => {
                render_preview(payload)
            }
            "remember" => render_stored(payload),
            // `admit --file` batch preview: summary counts, not one decision.
            "admit" if is_batch_preview(payload) => render_batch_preview(payload),
            "admit" => render_preview(payload),
            _ => render_job(payload),
        }
    }

    fn mutates(&self, verb: &str) -> bool {
        verb != "admit"
    }

    fn refusal(&self, verb: &str, payload: &Value) -> Option<Refusal> {
        // `admit` is the pre-flight check: a rejecting decision is the expected
        // output (exit 0, nothing written). A `remember` write maps `pass: false`
        // and a duplicate to the exit-1 domain refusal. A batch preview in which
        // every item is invalid is refused too, because nothing was previewed.
        match verb {
            "remember" => remember_refusal(payload),
            "admit" if is_batch_preview(payload) => batch_preview_refusal(payload),
            _ => None,
        }
    }
}

/// `remember TEXT --category C [--dry-run]`: `store`, or `admit_preview` on
/// `--dry-run` (the same gated decision, zero writes).
fn remember_call(parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    let args = candidate_args(parsed, "remember")?;
    let tool = if parsed.has("dry-run") {
        "admit_preview"
    } else {
        "store"
    };
    let mut args = args;
    // `--allow-duplicate` waives the exact-duplicate rejection only; every other
    // gate still runs. A dry run has nothing to waive, so it is dropped there.
    if tool == "store"
        && let Some(allow) = allow_duplicate_arg(parsed)
    {
        args.insert("allow_duplicate".into(), allow);
    }
    Ok(WriteCall::Tool {
        tool,
        args: Value::Object(args),
    })
}

/// `admit TEXT --category C` → `admit_preview`; `admit --file ops.json` →
/// `admit_preview_batch` (read-only batch preview over the file's `items`).
fn admit_call(parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    if let Some(path) = parsed.value("file") {
        return crate::cli_write_batch::admit_batch_call(parsed, path);
    }
    let args = candidate_args(parsed, "admit")?;
    Ok(WriteCall::Tool {
        tool: "admit_preview",
        args: Value::Object(args),
    })
}

/// The `--category` help block shared by `remember`/`admit` (a usage error,
/// exit 2). `message` stays a single line; this block is the `help` field.
///
/// The category list comes from `SemanticCategory::ALL`, the single source of
/// truth beside the whitelist, so this help cannot drift from the gate.
fn missing_category_help(verb: &str) -> String {
    let categories = SemanticCategory::ALL
        .iter()
        .map(|&(category, _wire, description)| {
            format!("  • {:<18}{description}", category.as_str())
        })
        .collect::<Vec<String>>()
        .join("\n");
    format!(
        "Usage: clio {verb} TEXT --category <category>\n\n\
         Valid Categories:\n{categories}\n\n\
         Example:\n  clio {verb} \"Bob lives in Canada\" --category persona"
    )
}

/// Shared candidate decoding for `remember`/`admit`: required `--category`,
/// fact/belief kind, and the mirrored optional flags.
///
/// `--category` stays required — no inference, no default. The
/// user-facing text names the option and lists valid values rather than citing
/// those internal requirement IDs.
fn candidate_args(parsed: &ParsedArgs, verb: &str) -> Result<Map<String, Value>, CliError> {
    let text = one_positional(parsed, verb, "TEXT")?;
    let raw_category = parsed.value("category").ok_or_else(|| {
        CliError::usage_help(
            "missing required option '--category <value>'",
            missing_category_help(verb),
        )
    })?;
    // Validated here, against the whitelist itself, rather than left for the
    // domain layer to reject (#39). A bad value for a caller-supplied flag is
    // the same class of mistake as a bad `--epistemic-kind` or an unknown flag,
    // and those two are already a usage error with exit 2. Letting the value
    // through unvalidated made this one flag the outlier: it reported
    // `invalid_argument` and exited 1, and it printed the category help block
    // for a *missing* value but not for an invalid one.
    let category = validate_choice("category", raw_category, SemanticCategory::WIRE)?;
    let kind = match parsed.value("epistemic-kind") {
        Some(raw) => validate_choice("epistemic-kind", raw, &["fact", "belief"])?,
        None => "fact",
    };
    let source_type = source_type_arg(kind, parsed)?;
    let mut args = base_args(parsed);
    args.insert("item".into(), json!({"id": caller_item_id(), "gist": text}));
    args.insert("category".into(), Value::from(category));
    args.insert("epistemic_kind".into(), Value::from(kind));
    if let Some(raw) = parsed.value("confidence") {
        args.insert(
            "confidence".into(),
            Value::from(parse_f64("confidence", raw)?),
        );
    }
    if let Some(source) = source_type {
        args.insert("source_type".into(), Value::from(source));
    }
    if let Some(evidence) = parsed.value("evidence-ref") {
        args.insert("evidence_ref".into(), Value::from(evidence));
    }
    if let Some(haystack) = parsed.value("source-text") {
        args.insert("source_text".into(), Value::from(haystack));
    }
    if let Some(context) = context_arg(parsed)? {
        args.insert("context".into(), Value::from(context));
    }
    Ok(args)
}

/// Decode the optional `--context` value shared by `remember`, `admit`, and
/// `correct`: a present value must be non-empty and at most
/// [`CONTEXT_MAX_BYTES`] UTF-8 bytes, and it is passed through unchanged (never
/// truncated).
///
/// The usage failure names the flag and the exact problem, and it never echoes
/// the raw value, so an oversized context is not copied into terminals or logs.
pub(crate) fn context_arg(parsed: &ParsedArgs) -> Result<Option<&str>, CliError> {
    let Some(raw) = parsed.value("context") else {
        return Ok(None);
    };
    validate_context(raw).map_err(|error| {
        let (problem, hint) = if error.code == ErrorCode::OutOfRange {
            (
                format!(
                    "must be at most {CONTEXT_MAX_BYTES} UTF-8 bytes, got {}",
                    raw.len()
                ),
                "context is never truncated; shorten it or omit --context",
            )
        } else {
            (
                "must not be empty when supplied (omit the flag instead)".to_owned(),
                "pass a non-empty value or omit --context",
            )
        };
        CliError::usage_hint(format!("--context {problem}"), hint)
    })?;
    Ok(Some(raw))
}

/// `summarize [--scope S] [--dry-run]`: regenerate gists for a scope; snapshots
/// are never altered. The in-process tool has no no-write path, so `--dry-run`
/// previews without calling the dispatcher.
fn summarize_call(parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    no_positionals(parsed)?;
    let scope = required_scope(parsed, "summarize")?;
    if parsed.has("dry-run") {
        return Ok(WriteCall::Preview {
            payload: json!({
                "dry_run": true,
                "scope": scope,
                "would": "regenerate gist(s) for the scope; snapshots are never altered"
            }),
            text: format!(
                "dry-run: would regenerate gist(s) for scope `{scope}` (snapshots are never altered)"
            ),
        });
    }
    let mut args = base_args(parsed);
    args.insert("scope".into(), Value::from(scope));
    Ok(WriteCall::Tool {
        tool: "summarize",
        args: Value::Object(args),
    })
}

/// `consolidate [--bank BANK] [--force] [--dry-run]`: trigger dirty-path
/// refresh and eligible hub distillation without blocking leaf writes.
///
/// The bank selector is the global `--bank`, so this builder never adds a
/// selector of its own: `base_args` carries it into the tool arguments.
fn consolidate_call(parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    no_positionals(parsed)?;
    let mut args = base_args(parsed);
    if parsed.has("force") {
        args.insert("force".into(), Value::Bool(true));
    }
    if parsed.has("dry-run") {
        return Ok(WriteCall::Preview {
            payload: json!({
                "dry_run": true,
                "bank": args.get("bank").cloned().unwrap_or(Value::Null),
                "force": parsed.has("force"),
                "would": "refresh dirty memtree ancestors and reconcile the bank's consolidated units; leaf writes are never blocked"
            }),
            text: format!(
                "dry-run: would refresh dirty memtree ancestors and reconcile consolidated units{} (leaf writes are never blocked)",
                if parsed.has("force") { " (force)" } else { "" }
            ),
        });
    }
    Ok(WriteCall::Tool {
        tool: "consolidate",
        args: Value::Object(args),
    })
}

fn required_scope<'a>(parsed: &'a ParsedArgs, verb: &str) -> Result<&'a str, CliError> {
    parsed.value("scope").ok_or_else(|| {
        CliError::usage_hint(
            format!("`{verb}` requires --scope (bank or item scope)"),
            format!("try `clio {verb} --scope default`"),
        )
    })
}

/// Unique item id for caller-omitted ids across one-shot CLI processes: the
/// in-process id counter resets on every process start, so it cannot be reused
/// here without colliding with the previous invocation's stored id.
pub(crate) fn caller_item_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!("itm-cli-{nanos}-{}", std::process::id())
}

fn render_stored(payload: &Value) -> String {
    let id = payload.get("id").and_then(Value::as_str).unwrap_or("?");
    match payload.get("admission_score").and_then(Value::as_f64) {
        Some(score) => format!("stored {id} (admission_score {score})"),
        None => format!("stored {id}"),
    }
}

/// The `admit --file` batch-preview payload (summary counts over the file's
/// `items`) rather than a single-item decision.
fn is_batch_preview(payload: &Value) -> bool {
    payload.get("summary").is_some() || payload.get("count").is_some()
}

/// The `admit --file` batch view: how many items would be admitted or
/// rejected, and that zero writes happened.
fn render_batch_preview(payload: &Value) -> String {
    let count = payload.get("count").and_then(Value::as_u64).unwrap_or(0);
    let summary = payload.get("summary");
    let would_admit = summary
        .and_then(|s| s.get("would_admit"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let would_reject = summary
        .and_then(|s| s.get("would_reject"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let writes = payload.get("writes").and_then(Value::as_u64).unwrap_or(0);
    format!(
        "preview: {} (would_admit: {would_admit}, would_reject: {would_reject}, writes: {writes})",
        crate::count_noun::count_noun(count, "item", "items")
    )
}

/// The `admit`/`--dry-run` decision view: pass/fail, score, factors, reason.
fn render_preview(payload: &Value) -> String {
    let pass = payload.get("pass").and_then(Value::as_bool);
    let mut lines = vec![match pass {
        Some(true) => "preview: pass".to_owned(),
        Some(false) => "preview: refused".to_owned(),
        None => "preview".to_owned(),
    }];
    if let Some(score) = payload.get("admission_score").and_then(Value::as_f64) {
        lines.push(format!("score: {score}"));
    }
    if let Some(factors) = payload.get("factors").and_then(Value::as_object) {
        let rendered: Vec<String> = factors
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        if !rendered.is_empty() {
            lines.push(format!("factors: {}", rendered.join(" ")));
        }
    }
    if let Some(reason) = payload.get("rejection_reason").and_then(Value::as_str) {
        lines.push(format!("reason: {reason}"));
    }
    lines.join("\n")
}

fn render_job(payload: &Value) -> String {
    let job = payload.get("job_id").and_then(Value::as_str).unwrap_or("?");
    let status = payload.get("status").and_then(Value::as_str).unwrap_or("?");
    let refreshed = payload.get("refreshed").and_then(Value::as_array);
    match refreshed {
        Some(ids) => format!("job {job} status: {status} refreshed: {}", ids.len()),
        None => format!("job {job} status: {status}"),
    }
}

#[cfg(test)]
#[path = "cli_write_core_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "cli_write_core_category_tests.rs"]
mod category_tests;

#[cfg(test)]
#[path = "cli_write_core_batch_tests.rs"]
mod batch_tests;

#[cfg(test)]
#[path = "cli_write_core_context_tests.rs"]
mod cli_write_core_context_tests;

#[cfg(test)]
#[path = "cli_write_core_context_parity_tests.rs"]
mod cli_write_core_context_parity_tests;
