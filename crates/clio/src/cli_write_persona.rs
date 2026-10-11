// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio` persona and history write
//! commands: `persona stable`, `persona observe`, `task upsert`, and
//! `failure record`, decoded into their gated MCP tools with `--dry-run`
//! no-write previews where the underlying tool supports one.
//!
//! ## Owns
//! - Flag decoding for these write verbs into MCP tool arguments
//! - Their human text views
//!
//! ## Does not own
//! - The write engine, parsing, and in-process call bridge (see `cli_write`)
//! - Persona/history semantics or the EMA engine (see `clio-persona`,
//!   `clio-history`, `clio-write`)
//!
//! ## Boundary
//!
//! Changes to these write commands' flags or arguments should normally require
//! changes here. Changes to persona, task, or failure semantics should not
//! require changes here.
//!
//! Keep this module focused on the persona and history write commands.

use crate::cli_args::{FlagSpec, ParsedArgs};
use crate::cli_error::CliError;
use crate::cli_read::{
    base_args, flag, one_positional, parse_u64, parse_unit_f64, require_flags, required_flags,
    validate_choice,
};
use crate::cli_write_group::{Refusal, WriteCall, WriteGroup, decision_refusal};
use serde_json::{Map, Value, json};

/// The persona and history write commands.
pub(crate) struct PersonaWriteGroup;

impl WriteGroup for PersonaWriteGroup {
    fn verbs(&self) -> &'static [&'static str] {
        &[
            "persona stable",
            "persona observe",
            "task upsert",
            "failure record",
        ]
    }

    fn spec(&self, verb: &str) -> FlagSpec {
        match verb {
            "persona stable" => FlagSpec::new(
                &["confidence", "source", "evidence-ref", "context"],
                &["dry-run"],
            ),
            // The EMA observation tool has no no-write path, so no `--dry-run`.
            "persona observe" => {
                FlagSpec::new(&["confidence", "source", "evidence-ref", "context"], &[])
            }
            "task upsert" => FlagSpec::new(
                &["definition", "status", "steps", "evidence-ref", "context"],
                &["dry-run"],
            ),
            _ => FlagSpec::new(
                &[
                    "attempt-n",
                    "what-failed",
                    "lesson",
                    "evidence-ref",
                    "context",
                ],
                &["dry-run"],
            ),
        }
    }

    fn build(&self, verb: &str, parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
        match verb {
            "persona stable" => stable_call(verb, parsed),
            "persona observe" => observe_call(verb, parsed),
            "task upsert" => task_upsert_call(verb, parsed),
            _ => failure_record_call(verb, parsed),
        }
    }

    fn render(&self, verb: &str, payload: &Value, _palette: crate::cli_style::Palette) -> String {
        match verb {
            "persona stable" => render_stable(payload),
            "persona observe" => render_observe(payload),
            "task upsert" => render_task_upsert(payload),
            _ => render_failure_record(payload),
        }
    }

    fn mutates(&self, _verb: &str) -> bool {
        true
    }

    fn refusal(&self, _verb: &str, payload: &Value) -> Option<Refusal> {
        // A dry-run preview is a read: it exits 0 even when the nested decision
        // rejected, mirroring `admit` (the preview is the output).
        if payload.get("outcome").and_then(Value::as_str) == Some("dry_run")
            || payload.get("dry_run").and_then(Value::as_bool) == Some(true)
        {
            return None;
        }
        decision_refusal(payload)
    }
}

/// `persona stable KEY VALUE [--confidence N] [--source explicit|inferred]
/// [--evidence-ref E] [--dry-run]` → `persona_put_stable`.
fn stable_call(verb: &str, parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    let positionals = parsed.positionals();
    match positionals {
        [key, value] => {
            let mut args = base_args(parsed);
            args.insert("key".into(), Value::from(key.as_str()));
            args.insert("value".into(), Value::from(value.as_str()));
            optional_flags(parsed, &mut args)?;
            if parsed.has("dry-run") {
                args.insert("dry_run".into(), Value::Bool(true));
            }
            Ok(WriteCall::Tool {
                tool: "persona_put_stable",
                args: Value::Object(args),
            })
        }
        [] | [_] => Err(CliError::usage_hint(
            format!("`{verb}` requires KEY and VALUE arguments"),
            format!("try 'clio {verb} KEY VALUE'"),
        )),
        _ => Err(unexpected_extra(verb, parsed, 2, "KEY VALUE")),
    }
}

/// `persona observe KEY NUMBER [--confidence N] [--source explicit|inferred]
/// [--evidence-ref E]` → `persona_observe_preference`. The EMA tool has no
/// no-write path, so no `--dry-run` exists for this verb.
fn observe_call(verb: &str, parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    let positionals = parsed.positionals();
    match positionals {
        [key, raw] => {
            // Fail closed: the EMA tool only accepts a finite observation. The
            // message names the positional, because there is no `--observed_value`
            // flag and a hint naming one could not be pasted and run.
            let observed = raw
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .ok_or_else(|| {
                    CliError::usage_hint(
                        format!("the NUMBER argument must be a finite number, got `{raw}`"),
                        format!("try `clio {verb} KEY 0.5`"),
                    )
                })?;
            let mut args = base_args(parsed);
            args.insert("key".into(), Value::from(key.as_str()));
            args.insert("observed_value".into(), json!(observed));
            optional_flags(parsed, &mut args)?;
            Ok(WriteCall::Tool {
                tool: "persona_observe_preference",
                args: Value::Object(args),
            })
        }
        [] | [_] => Err(CliError::usage_hint(
            format!("`{verb}` requires KEY and NUMBER arguments"),
            format!("usage: clio {verb} KEY NUMBER"),
        )),
        _ => Err(unexpected_extra(verb, parsed, 2, "KEY NUMBER")),
    }
}

/// `task upsert TASK_ID --definition D --status S [--steps a,b]
/// [--evidence-ref E] [--dry-run]` → `task_upsert`.
fn task_upsert_call(verb: &str, parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    let task_id = one_positional(parsed, verb, "TASK_ID")?;
    require_flags(parsed, verb, required_flags(verb))?;
    let mut args = base_args(parsed);
    args.insert("task_id".into(), Value::from(task_id));
    args.insert("definition".into(), Value::from(flag(parsed, "definition")));
    args.insert("status".into(), Value::from(flag(parsed, "status")));
    if let Some(raw) = parsed.value("steps") {
        let steps: Vec<Value> = raw
            .split(',')
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(Value::from)
            .collect();
        args.insert("steps".into(), Value::Array(steps));
    }
    if let Some(evidence) = parsed.value("evidence-ref") {
        args.insert("evidence_ref".into(), Value::from(evidence));
    }
    if let Some(context) = parsed.value("context") {
        args.insert("context".into(), Value::from(context));
    }
    if parsed.has("dry-run") {
        args.insert("dry_run".into(), Value::Bool(true));
    }
    Ok(WriteCall::Tool {
        tool: "task_upsert",
        args: Value::Object(args),
    })
}

/// `failure record TASK_ID --attempt-n N --what-failed W --lesson L
/// [--evidence-ref E] [--dry-run]` → `failure_record`.
fn failure_record_call(verb: &str, parsed: &ParsedArgs) -> Result<WriteCall, CliError> {
    let task_id = one_positional(parsed, verb, "TASK_ID")?;
    require_flags(parsed, verb, required_flags(verb))?;
    let mut args = base_args(parsed);
    args.insert("task_id".into(), Value::from(task_id));
    let attempt = parse_u64("attempt-n", flag(parsed, "attempt-n"))?;
    args.insert("attempt_n".into(), json!(attempt));
    args.insert(
        "what_failed".into(),
        Value::from(flag(parsed, "what-failed")),
    );
    args.insert("lesson".into(), Value::from(flag(parsed, "lesson")));
    if let Some(evidence) = parsed.value("evidence-ref") {
        args.insert("evidence_ref".into(), Value::from(evidence));
    }
    if let Some(context) = parsed.value("context") {
        args.insert("context".into(), Value::from(context));
    }
    if parsed.has("dry-run") {
        args.insert("dry_run".into(), Value::Bool(true));
    }
    Ok(WriteCall::Tool {
        tool: "failure_record",
        args: Value::Object(args),
    })
}

/// The persona flags `persona stable` and `persona observe` share.
fn optional_flags(parsed: &ParsedArgs, args: &mut Map<String, Value>) -> Result<(), CliError> {
    if let Some(raw) = parsed.value("confidence") {
        args.insert(
            "confidence".into(),
            json!(parse_unit_f64("confidence", raw)?),
        );
    }
    if let Some(source) = parsed.value("source") {
        args.insert(
            "source".into(),
            Value::from(validate_choice(
                "source",
                source,
                &["explicit", "inferred"],
            )?),
        );
    }
    if let Some(evidence) = parsed.value("evidence-ref") {
        args.insert("evidence_ref".into(), Value::from(evidence));
    }
    if let Some(context) = parsed.value("context") {
        args.insert("context".into(), Value::from(context));
    }
    Ok(())
}

fn unexpected_extra(verb: &str, parsed: &ParsedArgs, index: usize, accepts: &str) -> CliError {
    crate::cli_read_args::unexpected_extra(parsed, index, Some(verb), accepts)
}

fn render_stable(payload: &Value) -> String {
    let key = payload
        .pointer("/entry/key")
        .and_then(Value::as_str)
        .unwrap_or("?");
    match payload.get("outcome").and_then(Value::as_str) {
        Some("dry_run") => format!("dry-run: would upsert stable entry `{key}`"),
        _ => match payload
            .pointer("/decision/admission_score")
            .and_then(Value::as_f64)
        {
            Some(score) => format!("stored stable `{key}` (admission_score {score})"),
            None => format!("stored stable `{key}`"),
        },
    }
}

fn render_observe(payload: &Value) -> String {
    let key = payload.get("key").and_then(Value::as_str).unwrap_or("?");
    let state = payload
        .get("state")
        .map_or_else(|| "?".to_owned(), Value::to_string);
    let trend = payload.get("trend").and_then(Value::as_str).unwrap_or("?");
    format!("observed `{key}` state={state} trend={trend}")
}

fn render_task_upsert(payload: &Value) -> String {
    let task_id = payload
        .pointer("/record/task_id")
        .and_then(Value::as_str)
        .unwrap_or("?");
    match payload.get("outcome").and_then(Value::as_str) {
        Some("dry_run") => format!("dry-run: would store task `{task_id}`"),
        _ => format!("stored task `{task_id}`"),
    }
}

fn render_failure_record(payload: &Value) -> String {
    let task_id = payload
        .pointer("/record/task_id")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let attempt = payload
        .pointer("/record/attempt_n")
        .map_or_else(|| "?".to_owned(), Value::to_string);
    match payload.get("outcome").and_then(Value::as_str) {
        Some("dry_run") => {
            format!("dry-run: would record failure for task `{task_id}` (attempt {attempt})")
        }
        _ => format!("recorded failure for task `{task_id}` (attempt {attempt})"),
    }
}

#[cfg(test)]
#[path = "cli_write_persona_tests.rs"]
mod tests;
