// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio` write-command engine: parsing a
//! mutating verb (`remember`, `admit`, `summarize`, `consolidate`, and write
//! subcommands such as `triple add` and `persona stable`), opening runtime
//! state, and routing the call through the in-process MCP dispatcher so CLI and
//! MCP semantics cannot drift.
//!
//! ## Owns
//! - The write engine: flag parsing, `--help`, preview emission, and the
//!   in-process `tools/call` bridge for mutating verbs
//! - The exit-code mapping for writes: 0 ok, 1 refusal/operational/partial
//!   failure, 2 usage
//! - One bounded index drain after a successful mutating call (a one-shot
//!   process has no background sweeper)
//!
//! ## Does not own
//! - The `WriteGroup` contract (see `cli_write_group`)
//! - Any concrete command group's verbs or flags (see the `cli_write_*` modules)
//! - Gate or admission semantics (they run inside the in-process tools)
//!
//! ## Boundary
//!
//! Changes to the write-command engine should normally require changes here.
//! Changes to admission or storage semantics should not require changes here.
//!
//! Keep this module focused on the write-command engine.

use crate::cli_args::{FlagSpec, ParsedArgs};
use crate::cli_error::{CliError, EXIT_OK};
use crate::cli_help;
use crate::cli_output::{self, OutputMode};
use crate::cli_read::{CliIo, call_tool, emit, fail};
use crate::cli_write_group::{WriteCall, WriteGroup};
use clio_mcp::runtime::{McpOpenOptions, McpState};
use serde_json::Value;
use std::process::ExitCode;
use std::sync::Arc;

/// Top-level entrypoint for the mutating commands (`remember`, `admit`,
/// `summarize`, `consolidate`); group write verbs arrive via [`run_resolved`]
/// through the read engine's group resolution.
pub(crate) fn dispatch(cmd: &str, args: Vec<String>) -> ExitCode {
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    let mut input = std::io::stdin();
    let mut io = crate::cli_read::CliIo {
        out: &mut out,
        err: &mut err,
        stdin: &mut input,
        is_tty: cli_output::stdout_is_tty(),
        no_color: cli_output::no_color_requested(),
        env: &system_env,
    };
    run_resolved(cmd, args, &mut io)
}

/// Process environment lookup for the binary-facing dispatch path.
fn system_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// Parse and run one resolved write verb (the delegation seam for group
/// subcommands the read engine resolves, and for top-level write verbs).
pub(crate) fn run_resolved(verb: &str, args: Vec<String>, io: &mut CliIo) -> ExitCode {
    let fallback_mode = cli_output::mode_from_raw(&args, io.is_tty);
    let (parsed, mode) = match parse_verb(verb, args, io.is_tty) {
        Ok(parsed) => parsed,
        Err(error) => return fail(io, fallback_mode, &error),
    };
    if let Some(code) = help_if_requested(verb, &parsed, io) {
        return code;
    }
    let resolved =
        match crate::db_target::resolve_target(io.env, parsed.value("db"), parsed.value("backend"))
        {
            Ok(resolved) => resolved,
            Err(reason) => return fail(io, mode, &CliError::usage(reason)),
        };
    let kms_path = clio_config::kms_path::resolve_kms_path(&io.env, &resolved.url);
    let options = McpOpenOptions {
        backend: resolved.backend,
        database_url: resolved.url,
        // The runtime default is configuration, never the caller's `--bank`.
        // The request carries `--bank` in its own arguments, and promoting it
        // here would exempt the bank from a caller that checks it (the
        // summarize dry-run validates the named bank).
        default_bank: "default".to_owned(),
        default_actor: clio_types::Actor::Agent,
        // The shared surface is opt-in, mirroring `clio mcp stdio --shared-bank`;
        // without the flag the shared tools return the same Forbidden gate.
        shared_bank_enabled: parsed.has("shared-bank"),
        kms_path,
    };
    match McpState::open_one_shot(&options) {
        Ok(state) => run_parsed(verb, &parsed, mode, &state, io),
        Err(error) => fail(io, mode, &CliError::from_am_error(&error)),
    }
}

/// True when a write group owns `verb` (the read engine delegates these).
pub(crate) fn owns(verb: &str) -> bool {
    find_group(verb).is_some()
}

/// The write-command groups, in resolution order.
fn groups() -> [&'static dyn WriteGroup; 9] {
    [
        &crate::cli_write_core::CoreWriteGroup,
        &crate::cli_write_graph::GraphWriteGroup,
        &crate::cli_write_persona::PersonaWriteGroup,
        &crate::cli_write_workspace::WorkspaceWriteGroup,
        &crate::cli_write_shared::SharedWriteGroup,
        &crate::cli_write_batch::BatchWriteGroup,
        &crate::cli_write_mutate::MutationWriteGroup,
        &crate::cli_write_hygiene::HygieneWriteGroup,
        &crate::cli_write_portability::PortabilityWriteGroup,
    ]
}

/// The group that owns `verb`, if any.
/// The flag spec the parser will actually accept for `verb`, or `None` when the verb
/// belongs to no write group. See the read side for why a test needs this.
#[cfg(test)]
pub(crate) fn spec_for(verb: &str) -> Option<FlagSpec> {
    find_group(verb).map(|group| group.spec(verb))
}

fn find_group(verb: &str) -> Option<&'static dyn WriteGroup> {
    groups()
        .into_iter()
        .find(|group| group.verbs().contains(&verb))
}

fn parse_verb(
    verb: &str,
    args: Vec<String>,
    is_tty: bool,
) -> Result<(ParsedArgs, OutputMode), CliError> {
    let spec = find_group(verb).map_or_else(|| FlagSpec::new(&[], &[]), |group| group.spec(verb));
    let parsed = crate::cli_args::parse(&spec, args)
        .map_err(|error| crate::cli_error::with_help_hint(error, verb))?;
    let mode = cli_output::resolve_output(parsed.value("output"), is_tty)?;
    Ok((parsed, mode))
}

fn run_parsed(
    verb: &str,
    parsed: &ParsedArgs,
    mode: OutputMode,
    state: &Arc<McpState>,
    io: &mut CliIo,
) -> ExitCode {
    let Some(group) = find_group(verb) else {
        return fail(
            io,
            mode,
            &CliError::usage(format!("unknown command `{verb}`")),
        );
    };
    let call = match group.build_with_io(verb, parsed, io) {
        Ok(call) => call,
        Err(error) => return fail(io, mode, &error),
    };
    match call {
        WriteCall::Preview { payload, text } => {
            if verb == "summarize"
                && let Err(error) = validate_summarize_preview(state, parsed)
            {
                return fail(io, mode, &error);
            }
            emit(io, mode, &payload, &text);
            ExitCode::from(EXIT_OK)
        }
        WriteCall::Tool { tool, args } => match call_tool(state, tool, &args) {
            Ok(payload) => {
                if let Some(refusal) = group.refusal(verb, &payload) {
                    return fail(
                        io,
                        mode,
                        &CliError::operational(refusal.code, refusal.message),
                    );
                }
                let text = group.render(
                    verb,
                    &payload,
                    crate::cli_style::Palette::new(cli_output::supports_color(
                        io.is_tty,
                        io.no_color,
                    )),
                );
                emit(io, mode, &payload, &text);
                drain_after_write(group, verb, state, &args, &payload);
                match group.failure_exit(verb, &payload) {
                    Some(code) => ExitCode::from(code),
                    None => ExitCode::from(EXIT_OK),
                }
            }
            Err(error) => fail(io, mode, &error),
        },
    }
}

fn validate_summarize_preview(state: &McpState, parsed: &ParsedArgs) -> Result<(), CliError> {
    let mut args = serde_json::Map::new();
    if let Some(bank) = parsed.value("bank") {
        args.insert("bank".into(), serde_json::Value::from(bank));
    }
    let ctx = state
        .resolve_ctx(&serde_json::Value::Object(args))
        .map_err(|error| CliError::from_am_error(&error))?;
    state
        .require_known_bank(ctx.bank.as_str())
        .map_err(|error| CliError::from_am_error(&error))?;
    let Some(scope) = parsed.value("scope") else {
        return Ok(());
    };
    if scope == "bank"
        || state
            .store
            .get_memory_item(ctx.bank.as_str(), scope)
            .map_err(|error| CliError::from_am_error(&error))?
            .is_some()
    {
        return Ok(());
    }
    Err(CliError::operational(
        "item_not_found",
        format!("item `{scope}` not found in bank"),
    ))
}

/// Drain the index work a one-shot write just enqueued so the item is
/// searchable for the next command. The general backlog drain stays bounded;
/// the item this write stored is drained on its own, so a full batch of older
/// jobs cannot leave it unindexed. The response is already emitted; a drain
/// failure never fails the write (it stays on the durable job row).
fn drain_after_write(
    group: &dyn WriteGroup,
    verb: &str,
    state: &Arc<McpState>,
    args: &Value,
    payload: &Value,
) {
    if !group.mutates(verb) {
        return;
    }
    if let Err(error) = state.drain_index() {
        clio_config::log::warn("index_drain_failed", &[("error", error.message.as_str())]);
    }
    drain_written_item(state, args, payload);
}

/// Index the item this write stored, even when an older backlog filled the
/// bounded batch drain. Best-effort: a failure stays on the durable job row for
/// the next drain.
fn drain_written_item(state: &Arc<McpState>, args: &Value, payload: &Value) {
    // Resolve the bank the way the tool did, so the targeted job matches.
    let (Some(item_id), Ok(ctx)) = (
        payload.get("id").and_then(Value::as_str),
        state.resolve_ctx(args),
    ) else {
        return;
    };
    if let Err(error) = state.drain_index_item(ctx.bank.as_str(), item_id) {
        clio_config::log::warn(
            "index_item_drain_failed",
            &[("error", error.message.as_str())],
        );
    }
}

fn help_if_requested(verb: &str, parsed: &ParsedArgs, io: &mut CliIo) -> Option<ExitCode> {
    if !parsed.has("help") {
        return None;
    }
    let (usage, tool) = cli_help::verb_help(verb)?;
    let _ = cli_output::write_text(
        &mut io.out,
        &format!("{usage}\n\n{verb} calls the `{tool}` tool."),
    );
    Some(ExitCode::from(EXIT_OK))
}

#[cfg(test)]
pub(crate) fn run_with_state(
    verb: &str,
    args: Vec<String>,
    state: &Arc<McpState>,
    io: &mut CliIo,
) -> ExitCode {
    let fallback_mode = cli_output::mode_from_raw(&args, io.is_tty);
    let (parsed, mode) = match parse_verb(verb, args, io.is_tty) {
        Ok(parsed) => parsed,
        Err(error) => return fail(io, fallback_mode, &error),
    };
    if let Some(code) = help_if_requested(verb, &parsed, io) {
        return code;
    }
    run_parsed(verb, &parsed, mode, state, io)
}

#[cfg(test)]
#[path = "cli_write_tests.rs"]
mod tests;
