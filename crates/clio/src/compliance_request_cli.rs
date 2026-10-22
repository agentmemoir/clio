// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio compliance` durable-request verbs:
//! it decodes each request command's flags, calls the matching in-process
//! `subject_request_*` tool, and renders the separated request status.
//!
//! ## Owns
//! - Flag decoding for the request and peer-request commands (1:1 with the
//!   published `inputSchema`)
//! - The store open, the in-process tool call, and the exit codes
//! - The text view of the separated request fields
//!
//! ## Does not own
//! - The command records and their flags (see
//!   `cli_command_registry_compliance`)
//! - The request state machine and the durable stores (see `clio-compliance`)
//! - The in-process call bridge (see `cli_read`) or argument parsing (see
//!   `cli_args`)
//!
//! ## Boundary
//!
//! Changes to a request command's flags or text view should normally require
//! changes here. Changes to the request state machine or the tool contract
//! should not.
//!
//! Keep this module focused on the CLI request binding.

use crate::cli_args::{self, FlagSpec, ParsedArgs};
use crate::cli_error::{CliError, EXIT_OK};
use crate::cli_output::{self, OutputMode};
use crate::cli_read::{
    CliIo, base_args, call_tool, emit, fail, flag, no_positionals, parse_u64, require_flags,
    required_flags, validate_choice,
};
use clio_mcp::runtime::{McpOpenOptions, McpState};
use serde_json::{Map, Value};
use std::process::ExitCode;
use std::sync::Arc;

/// Run one `compliance` request verb: parse its record's flags, open the store,
/// call the matching tool, and render the result.
pub(crate) fn run_request(
    io: &mut CliIo,
    fallback: OutputMode,
    sub: &str,
    tokens: Vec<String>,
) -> ExitCode {
    let (parsed, mode) = match parse_request(sub, tokens, io.is_tty) {
        Ok(parsed) => parsed,
        Err(error) => return fail(io, fallback, &error),
    };
    let state = match open_state(io, &parsed) {
        Ok(state) => state,
        Err(error) => return fail(io, mode, &error),
    };
    execute(sub, &parsed, mode, &state, io)
}

/// Parse and run one request verb against injected state (the test seam).
#[cfg(test)]
pub(crate) fn run_with_state(
    sub: &str,
    args: Vec<String>,
    state: &Arc<McpState>,
    io: &mut CliIo,
) -> ExitCode {
    let fallback = cli_output::mode_from_raw(&args, io.is_tty);
    let (parsed, mode) = match parse_request(sub, args, io.is_tty) {
        Ok(parsed) => parsed,
        Err(error) => return fail(io, fallback, &error),
    };
    execute(sub, &parsed, mode, state, io)
}

/// Resolve one request verb's record, parse its flags, and resolve the output
/// mode.
fn parse_request(
    sub: &str,
    tokens: Vec<String>,
    is_tty: bool,
) -> Result<(ParsedArgs, OutputMode), CliError> {
    let command = format!("compliance {sub}");
    let spec = crate::cli_command_registry::find(&command).map_or_else(
        || FlagSpec::new(&[], &[]),
        crate::cli_command_spec::CommandSpec::parser_spec,
    );
    let parsed = cli_args::parse(&spec, tokens)
        .map_err(|error| crate::cli_error::with_help_hint(error, &command))?;
    let mode = cli_output::resolve_output(parsed.value("output"), is_tty)?;
    Ok((parsed, mode))
}

/// Open runtime state through the shared database precedence resolver.
fn open_state(io: &CliIo, parsed: &ParsedArgs) -> Result<Arc<McpState>, CliError> {
    let get = |key: &str| (io.env)(key);
    let target =
        crate::db_target::resolve_target(&get, parsed.value("db"), parsed.value("backend"))
            .map_err(CliError::usage)?;
    let options = McpOpenOptions {
        backend: target.backend,
        database_url: target.url.clone(),
        default_bank: "default".to_owned(),
        default_actor: clio_types::Actor::Agent,
        shared_bank_enabled: false,
        kms_path: clio_config::kms_path::resolve_kms_path(&get, &target.url),
    };
    McpState::open_one_shot(&options).map_err(|error| CliError::from_am_error(&error))
}

/// Validate the parsed flags, build the tool call, and render its result.
fn execute(
    sub: &str,
    parsed: &ParsedArgs,
    mode: OutputMode,
    state: &Arc<McpState>,
    io: &mut CliIo,
) -> ExitCode {
    if let Err(error) = validate(sub, parsed) {
        return fail(io, mode, &error);
    }
    let (tool, args) = match build_request_call(sub, parsed) {
        Ok(call) => call,
        Err(error) => return fail(io, mode, &error),
    };
    match call_tool(state, tool, &args) {
        Ok(payload) => {
            emit(io, mode, &payload, &render(sub, &payload));
            ExitCode::from(EXIT_OK)
        }
        Err(error) => fail(io, mode, &error),
    }
}

/// Check every required flag is present and no positional was given.
fn validate(sub: &str, parsed: &ParsedArgs) -> Result<(), CliError> {
    let command = format!("compliance {sub}");
    require_flags(parsed, &command, required_flags(&command))?;
    no_positionals(parsed)
}

/// Build the tool name and arguments for one request verb.
fn build_request_call(sub: &str, parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError> {
    let mut args = base_args(parsed);
    match sub {
        "request-start" => {
            let kind = validate_choice("kind", flag(parsed, "kind"), &["copy", "erasure"])?;
            args.insert("request_id".into(), required(parsed, "request-id"));
            args.insert("kind".into(), Value::from(kind));
            insert_opt(&mut args, parsed, "subject-id", "subject_id");
            insert_opt(&mut args, parsed, "verification", "verification");
            insert_opt(&mut args, parsed, "deadline", "deadline");
            Ok(("subject_request_start", Value::Object(args)))
        }
        "request-status" => {
            args.insert("request_id".into(), required(parsed, "request-id"));
            Ok(("subject_request_status", Value::Object(args)))
        }
        "request-retry" => {
            args.insert("request_id".into(), required(parsed, "request-id"));
            args.insert("subject_id".into(), required(parsed, "subject-id"));
            args.insert("legal_basis".into(), required(parsed, "legal-basis"));
            insert_opt(&mut args, parsed, "reason-code", "reason_code");
            args.insert("confirm".into(), Value::Bool(parsed.has("confirm")));
            Ok(("subject_request_retry", Value::Object(args)))
        }
        "request-copy" => {
            let custody = validate_choice(
                "custody",
                flag(parsed, "custody"),
                &[
                    "clio",
                    "operator",
                    "caller",
                    "peer",
                    "review_output",
                    "unknown",
                ],
            )?;
            let owner = validate_choice("owner", flag(parsed, "owner"), &["harness", "system"])?;
            let action = validate_choice(
                "required-action",
                flag(parsed, "required-action"),
                &["erase", "remove", "delivery", "notice"],
            )?;
            args.insert("copy_id".into(), required(parsed, "copy-id"));
            args.insert("request_id".into(), required(parsed, "request-id"));
            args.insert("custody".into(), Value::from(custody));
            args.insert("owner".into(), Value::from(owner));
            args.insert("required_action".into(), Value::from(action));
            args.insert("state".into(), required(parsed, "state"));
            insert_opt(&mut args, parsed, "exception-reason", "exception_reason");
            insert_opt(&mut args, parsed, "updated-at", "updated_at");
            Ok(("subject_request_copy", Value::Object(args)))
        }
        "request-exception" => {
            args.insert("request_id".into(), required(parsed, "request-id"));
            args.insert("reason".into(), required(parsed, "reason"));
            Ok(("subject_request_exception", Value::Object(args)))
        }
        "request-events" => {
            args.insert("request_id".into(), required(parsed, "request-id"));
            if let Some(limit) = parsed.value("limit") {
                args.insert("limit".into(), Value::from(parse_u64("limit", limit)?));
            }
            insert_opt(&mut args, parsed, "cursor", "cursor");
            Ok(("subject_request_events", Value::Object(args)))
        }
        "request-path-complete" | "request-close" => {
            args.insert("request_id".into(), required(parsed, "request-id"));
            args.insert("product_path".into(), required(parsed, "product-path"));
            let tool = if sub == "request-close" {
                "subject_request_close"
            } else {
                "subject_request_path_complete"
            };
            Ok((tool, Value::Object(args)))
        }
        "peer-status" => {
            args.insert("request_id".into(), required(parsed, "request-id"));
            Ok(("subject_request_peer_status", Value::Object(args)))
        }
        "peer-retry" => {
            args.insert("request_id".into(), required(parsed, "request-id"));
            insert_opt(&mut args, parsed, "legal-basis", "legal_basis");
            Ok(("subject_request_peer_retry", Value::Object(args)))
        }
        "peer-disposition" => {
            let disposition = validate_choice(
                "disposition",
                flag(parsed, "disposition"),
                &["removal_evidence", "retention_approved", "unknown"],
            )?;
            args.insert("request_id".into(), required(parsed, "request-id"));
            args.insert("peer_id".into(), required(parsed, "peer-id"));
            args.insert("disposition".into(), Value::from(disposition));
            insert_opt(&mut args, parsed, "proof-ref", "proof_ref");
            Ok(("subject_request_peer_disposition", Value::Object(args)))
        }
        other => Err(CliError::operational(
            "internal",
            format!("no request binding for `{other}`"),
        )),
    }
}

/// A required flag's value; the required set was already checked.
fn required(parsed: &ParsedArgs, name: &str) -> Value {
    Value::from(flag(parsed, name))
}

/// Insert an optional flag's value under its tool argument name.
fn insert_opt(args: &mut Map<String, Value>, parsed: &ParsedArgs, name: &str, key: &str) {
    if let Some(value) = parsed.value(name) {
        args.insert(key.to_owned(), Value::from(value));
    }
}

/// Render one verb's payload as its text view.
fn render(sub: &str, payload: &Value) -> String {
    match sub {
        "request-retry" => render_retry(payload),
        "peer-status" => render_peers(payload),
        "request-events" => render_events(payload),
        _ => render_request(payload),
    }
}

/// Render the separated request fields: state, technical completion, pending
/// operator actions, and request outcome, each its own field. The `erase` verb
/// renders its request report through this same view.
pub(crate) fn render_request(payload: &Value) -> String {
    let id = payload
        .get("request_id")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let state = state_label(payload.get("state").unwrap_or(&Value::Null));
    let completion = payload
        .get("technical_completion")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let pending = pending_label(payload.get("pending_operator_actions"));
    let outcome = payload
        .get("request_outcome")
        .and_then(Value::as_str)
        .unwrap_or("?");
    format!(
        "request {id}: state={state}; technical_completion={completion}; \
         pending_operator_actions={pending}; request_outcome={outcome}"
    )
}

/// Render a retry: the request fields plus the retried action and its effect
/// counts.
fn render_retry(payload: &Value) -> String {
    let action = payload
        .get("action_id")
        .and_then(Value::as_str)
        .unwrap_or("none");
    let before = payload
        .get("effects_before")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let after = payload
        .get("effects_after")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    format!(
        "{}; retry_action={action}; effects={before}->{after}",
        render_request(payload)
    )
}

/// Render the peer slots of one request.
fn render_peers(payload: &Value) -> String {
    let mut peers: Vec<String> = payload
        .as_object()
        .map(|map| {
            map.iter()
                .filter(|(key, _)| key.as_str() != "ok")
                .map(|(peer, state)| format!("{peer}={}", state_label(state)))
                .collect()
        })
        .unwrap_or_default();
    peers.sort_unstable();
    if peers.is_empty() {
        "peers: none".to_owned()
    } else {
        format!("peers: {}", peers.join(", "))
    }
}

/// Render one content-free event page.
fn render_events(payload: &Value) -> String {
    let rows = payload.get("rows").and_then(Value::as_array);
    let count = rows.map_or(0, Vec::len);
    let has_more = payload
        .get("has_more")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let cursor = payload
        .get("next_cursor")
        .and_then(Value::as_str)
        .unwrap_or("none");
    let mut lines = vec![format!(
        "events: {count} row(s); has_more={has_more}; next_cursor={cursor}"
    )];
    for row in rows.into_iter().flatten() {
        let seq = row
            .get("seq")
            .map_or_else(|| "?".to_owned(), Value::to_string);
        let action = row.get("action_id").and_then(Value::as_str).unwrap_or("?");
        let state = row.get("state").and_then(Value::as_str).unwrap_or("?");
        lines.push(format!("  {seq} {action} {state}"));
    }
    lines.join("\n")
}

/// The text spelling of one request state: a plain string, or the single-key
/// object form (`{final: completed}`, `{blocked: {reason}}`).
fn state_label(state: &Value) -> String {
    match state {
        Value::String(text) => text.clone(),
        Value::Object(map) if map.len() == 1 => {
            let (key, inner) = map.iter().next().expect("one entry");
            if let Some(text) = inner.as_str() {
                format!("{key}:{text}")
            } else {
                let detail = inner
                    .get("reason")
                    .or_else(|| inner.get("rationale"))
                    .and_then(Value::as_str);
                detail.map_or_else(|| key.clone(), |detail| format!("{key}({detail})"))
            }
        }
        other => other.to_string(),
    }
}

/// Render the pending operator actions as `action_id(kind)` pairs, where the
/// kind is shown only when the action carries one, or `none`.
fn pending_label(value: Option<&Value>) -> String {
    let Some(items) = value.and_then(Value::as_array) else {
        return "none".to_owned();
    };
    if items.is_empty() {
        return "none".to_owned();
    }
    items
        .iter()
        .map(|item| {
            let id = item.get("action_id").and_then(Value::as_str).unwrap_or("?");
            match item.get("kind").and_then(Value::as_str) {
                Some(kind) => format!("{id}({kind})"),
                None => id.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
#[path = "compliance_request_cli_tests.rs"]
mod compliance_request_cli_tests;

#[cfg(test)]
#[path = "compliance_request_cli_scenario_tests.rs"]
mod compliance_request_cli_scenario_tests;

#[cfg(test)]
#[path = "compliance_request_cli_restart_tests.rs"]
mod compliance_request_cli_restart_tests;

#[cfg(test)]
#[path = "compliance_request_cli_scope_tests.rs"]
mod compliance_request_cli_scope_tests;

#[cfg(test)]
#[path = "compliance_request_cli_pg_tests.rs"]
mod compliance_request_cli_pg_tests;

#[cfg(test)]
#[path = "compliance_request_cli_pg_restart_tests.rs"]
mod compliance_request_cli_pg_restart_tests;

#[cfg(test)]
#[path = "compliance_request_cli_pg_scope_tests.rs"]
mod compliance_request_cli_pg_scope_tests;
