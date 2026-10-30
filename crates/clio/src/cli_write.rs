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
//! - The write receipt's bank naming: the top-level `bank` result field, the
//!   `bank_source` fallback disclosure, and the text-mode `bank:` lines
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
        // summarize and consolidate dry-runs validate the named bank).
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
        WriteCall::Preview { mut payload, text } => {
            if verb == "summarize" || verb == "consolidate" {
                let counts = crate::cli_write_preview::validate_preview(state, verb, parsed)
                    .and_then(|bank| {
                        crate::cli_write_preview::preview_counts(state, verb, parsed, &bank)
                    });
                match counts {
                    Ok(counts) => payload["counts"] = counts,
                    Err(error) => return fail(io, mode, &error),
                }
            }
            emit(io, mode, &payload, &text);
            ExitCode::from(EXIT_OK)
        }
        WriteCall::Tool { tool, args } => match call_tool(state, tool, &args) {
            Ok(mut payload) => {
                if let Some(refusal) = group.refusal(verb, &payload) {
                    return fail(
                        io,
                        mode,
                        &CliError::operational(refusal.code, refusal.message),
                    );
                }
                let mut text = group.render(
                    verb,
                    &payload,
                    crate::cli_style::Palette::new(cli_output::supports_color(
                        io.is_tty,
                        io.no_color,
                    )),
                );
                if group.mutates(verb)
                    && let Some((bank, defaulted)) = receipt_bank(verb, &args, state)
                {
                    name_receipt_bank(&mut payload, bank, defaulted);
                    let receipt = receipt_bank_lines(bank, defaulted);
                    text = format!("{receipt}{text}");
                }
                emit(io, mode, &payload, &text);
                drain_after_write(group, verb, state, &payload);
                match group.failure_exit(verb, &payload) {
                    Some(code) => ExitCode::from(code),
                    None => ExitCode::from(EXIT_OK),
                }
            }
            Err(error) => fail(io, mode, &error),
        },
    }
}

/// The bank a write receipt names, and whether it was a fallback: the caller's
/// `--bank`, or the CLI default with `defaulted` set when no bank was named.
///
/// Only a command whose help documents `--bank` gets the naming, so a command
/// that selects its bank another way (`--scope` for `summarize`, `--shared-bank`
/// for the shared surface) returns `None` and keeps its receipt unchanged.
fn receipt_bank<'a>(verb: &str, args: &'a Value, state: &'a McpState) -> Option<(&'a str, bool)> {
    let advertises_bank = crate::cli_command_registry::find(verb)
        .is_some_and(|spec| spec.usage_line().contains("--bank"));
    if !advertises_bank {
        return None;
    }
    let named = args.get("bank").and_then(Value::as_str);
    Some((named.unwrap_or(&state.default_bank), named.is_none()))
}

/// Put the effective bank on the write result where a JSON reader cannot miss
/// it: a top-level `bank` beside the per-item `written` entries, and when the
/// caller named no bank, the `bank_source` disclosure that a fallback was used.
/// A bank the tool itself reported stays untouched.
fn name_receipt_bank(payload: &mut Value, bank: &str, defaulted: bool) {
    let Some(fields) = payload.as_object_mut() else {
        return;
    };
    fields
        .entry("bank".to_owned())
        .or_insert_with(|| Value::from(bank));
    if defaulted {
        fields
            .entry("bank_source".to_owned())
            .or_insert_with(|| Value::from("defaulted"));
    }
}

/// The text-mode receipt lines that name the bank: one `bank:` line at the top,
/// and when the caller named no bank, the sentence that says the fallback was
/// used, so a write that landed in the default bank is never mistaken for data
/// in the bank a later read targets.
fn receipt_bank_lines(bank: &str, defaulted: bool) -> String {
    let mut lines = format!("bank: {bank}\n");
    if defaulted {
        lines.push_str("notice: no --bank was named; this write used the fallback bank `");
        lines.push_str(bank);
        lines.push_str("`\n");
    }
    lines
}

/// Drain the index work a one-shot write just enqueued so the items are
/// searchable for the next command. The general backlog drain stays bounded;
/// the items this write reported are drained on their own, so a full batch of
/// older jobs cannot leave them unindexed. The response is already emitted; a
/// drain failure never fails the write (it stays on the durable job row).
fn drain_after_write(group: &dyn WriteGroup, verb: &str, state: &Arc<McpState>, payload: &Value) {
    if !group.mutates(verb) {
        return;
    }
    if let Err(error) = state.drain_index() {
        clio_config::log::warn("index_drain_failed", &[("error", error.message.as_str())]);
    }
    drain_written_items(state, payload);
}

/// Index the items this write reported, even when an older backlog filled the
/// bounded batch drain. The write tool reports, per item, the bank it landed in
/// and the item id (`written`), so this never resolves either again; a payload
/// without `written` wrote nothing to target.
///
/// Best-effort, and bounded by the retry backoff: an item whose job just failed
/// is left on its durable row for a later drain rather than retried at once.
fn drain_written_items(state: &Arc<McpState>, payload: &Value) {
    let Some(written) = payload.get("written").and_then(Value::as_array) else {
        return;
    };
    for item in written {
        let (Some(bank), Some(id)) = (
            item.get("bank").and_then(Value::as_str),
            item.get("id").and_then(Value::as_str),
        ) else {
            continue;
        };
        if let Err(error) = state.drain_index_item(bank, id) {
            clio_config::log::warn(
                "index_item_drain_failed",
                &[("error", error.message.as_str())],
            );
        }
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
