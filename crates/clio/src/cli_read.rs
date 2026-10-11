// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio` read-command engine: resolving a
//! command (including two-token groups such as `task history`), parsing its
//! flags, opening runtime state, and routing the call through the in-process MCP
//! dispatcher so CLI and MCP semantics cannot drift. Group-resolved write
//! subcommands (e.g. `triple add`) delegate to the write engine.
//!
//! ## Owns
//! - Command resolution, flag parsing, and the in-process `tools/call` bridge
//! - Opening runtime state through the shared database precedence resolver
//! - Shared read helpers (positionals, numeric/enum flags, base arguments) and
//!   the read exit codes
//!
//! ## Does not own
//! - Any concrete command group's verbs, flags, or text views (see the
//!   `cli_read_*` modules)
//! - The parser/output/error contracts (see `cli_args` / `cli_output` / `cli_error`)
//!
//! ## Boundary
//!
//! Changes to the read-command engine should normally require changes here.
//! Changes to a specific command group should not require changes here.
//!
//! Keep this module focused on the read-command engine.

use crate::cli_args::{self, FlagSpec, ParsedArgs};
use crate::cli_error::{CliError, EXIT_OK};
use crate::cli_help;
use crate::cli_output::{self, OutputMode};
use crate::cli_read_group::ReadGroup;
use clio_mcp::runtime::{McpOpenOptions, McpState};
use serde_json::{Value, json};
use std::process::ExitCode;
use std::sync::Arc;

/// The shared read-command argument helpers. Re-exported here because every
/// command group reaches them through this module, and the split into
/// `cli_read_args` exists for size, not for a second import path.
pub(crate) use crate::cli_read_args::{
    base_args, flag, no_positionals, one_positional, parse_f64, parse_u64, parse_unit_f64,
    read_args, require_flags, required_flags, validate_choice, validate_time_axis,
};

/// Injected streams, terminal state, and environment for one read command.
pub(crate) struct CliIo<'a> {
    pub out: &'a mut dyn std::io::Write,
    pub err: &'a mut dyn std::io::Write,
    /// Injectable stdin (read commands never read it; `batch --file -` does).
    pub stdin: &'a mut dyn std::io::Read,
    pub is_tty: bool,
    pub no_color: bool,
    pub env: &'a dyn Fn(&str) -> Option<String>,
}

pub(crate) fn dispatch(cmd: &str, args: Vec<String>) -> ExitCode {
    let env = |key: &str| std::env::var(key).ok();
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    let mut input = std::io::stdin();
    let mut io = CliIo {
        out: &mut out,
        err: &mut err,
        stdin: &mut input,
        is_tty: cli_output::stdout_is_tty(),
        no_color: cli_output::no_color_requested(),
        env: &env,
    };
    run_verb(cmd, args, &mut io)
}

pub(crate) fn run_verb(cmd: &str, args: Vec<String>, io: &mut CliIo) -> ExitCode {
    let fallback_mode = cli_output::mode_from_raw(&args, io.is_tty);
    if let Some(code) = group_help_if_requested(cmd, &args, io) {
        return code;
    }
    let (verb, args) = match resolve_verb(cmd, args) {
        Ok(resolved) => resolved,
        Err(error) => return fail(io, fallback_mode, &error),
    };
    // Write subcommands (e.g. `triple add`) share group resolution but route
    // through the write engine's exit-code and dry-run contract.
    if crate::cli_write::owns(&verb) {
        return crate::cli_write::run_resolved(&verb, args, io);
    }
    let (parsed, mode) = match parse_verb(&verb, args, io.is_tty) {
        Ok(parsed) => parsed,
        Err(error) => return fail(io, fallback_mode, &error),
    };
    if let Some(code) = help_if_requested(&verb, &parsed, io) {
        return code;
    }
    let get = |key: &str| (io.env)(key);
    let resolved =
        match crate::db_target::resolve_target(io.env, parsed.value("db"), parsed.value("backend"))
        {
            Ok(resolved) => resolved,
            Err(reason) => return fail(io, mode, &CliError::usage(reason)),
        };
    let kms_path = clio_config::kms_path::resolve_kms_path(&get, &resolved.url);
    let options = McpOpenOptions {
        backend: resolved.backend,
        database_url: resolved.url,
        // The configured default bank, never the caller's `--bank`: the request
        // already carries `--bank` in its own arguments, and promoting it to the
        // runtime default would exempt it from the known-bank check, so an
        // unknown bank would read as an empty page instead of an error.
        default_bank: "default".to_owned(),
        default_actor: clio_types::Actor::Agent,
        shared_bank_enabled: false,
        kms_path,
    };
    match McpState::open_one_shot(&options) {
        Ok(state) => run_parsed(&verb, &parsed, mode, &state, io),
        Err(error) => fail(io, mode, &CliError::from_am_error(&error)),
    }
}

/// Parse and run the verb against injected state (the test seam).
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

/// Resolve a command into its canonical verb key plus the remaining arguments.
///
/// A group command (`task`, `failure`, `memtree`, `history`, `triple`, `belief`,
/// `graph`, `persona`, `intent`, `maintenance`, `audit`) takes its subcommand as
/// the first argument; every other command is already canonical.
fn resolve_verb(cmd: &str, args: Vec<String>) -> Result<(String, Vec<String>), CliError> {
    let Some(subcommands) = cli_help::group_subcommands(cmd) else {
        return Ok((cmd.to_owned(), args));
    };
    let first = subcommands.first().copied().unwrap_or_default();
    match args.first().map(String::as_str) {
        Some(sub) if subcommands.contains(&sub) => Ok((format!("{cmd} {sub}"), args[1..].to_vec())),
        Some(sub) if !sub.starts_with('-') => {
            let hint = cli_args::suggest(sub, subcommands).map_or_else(
                || format!("try `clio {cmd} {first}`"),
                |best| format!("did you mean `clio {cmd} {best}`?"),
            );
            Err(CliError::usage(format!("unknown `{cmd}` subcommand `{sub}`")).with_hint(hint))
        }
        _ => Err(CliError::usage_hint(
            format!("`{cmd}` requires a subcommand"),
            format!("try `clio {cmd} {first}`"),
        )),
    }
}

/// Render a group's `--help` (its subcommands and the tool each binds).
fn group_help_if_requested(cmd: &str, args: &[String], io: &mut CliIo) -> Option<ExitCode> {
    let subcommands = cli_help::group_subcommands(cmd)?;
    let first = args.first()?;
    if first != "--help" && first != "-h" {
        return None;
    }
    let _ = cli_output::write_text(&mut io.out, &cli_help::group_help(cmd, subcommands));
    Some(ExitCode::from(EXIT_OK))
}

fn parse_verb(
    verb: &str,
    args: Vec<String>,
    is_tty: bool,
) -> Result<(ParsedArgs, OutputMode), CliError> {
    let spec = find_group(verb).map_or_else(|| FlagSpec::new(&[], &[]), |group| group.spec(verb));
    let parsed = cli_args::parse(&spec, args)
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
    let (tool, arguments) = match group.build(verb, parsed) {
        Ok(call) => call,
        Err(error) => return fail(io, mode, &error),
    };
    match call_tool(state, tool, &arguments) {
        Ok(mut payload) => {
            group.decorate(verb, parsed, &mut payload);
            let text = group.render(
                verb,
                &payload,
                crate::cli_style::Palette::new(cli_output::supports_color(io.is_tty, io.no_color)),
            );
            emit(io, mode, &payload, &text);
            ExitCode::from(EXIT_OK)
        }
        Err(error) => fail(io, mode, &error),
    }
}

/// The read-command groups, in resolution order.
fn groups() -> [&'static dyn ReadGroup; 6] {
    [
        &crate::cli_read_core::CoreGroup,
        &crate::cli_read_history::HistoryGroup,
        &crate::cli_read_belief::BeliefGroup,
        &crate::cli_read_graph::GraphGroup,
        &crate::cli_read_workspace::WorkspaceGroup,
        &crate::cli_read_hygiene::HygieneReadGroup,
    ]
}

/// The group that owns `verb`, if any.
/// The flag spec the parser will actually accept for `verb`, or `None` when the verb
/// belongs to no read group.
///
/// Exposed so a test can ask the parser what it takes, rather than asking the usage
/// registry what it says: a flag named in a usage line that the parser would refuse
/// is the defect a reader cannot recover from, and only the parser knows.
#[cfg(test)]
pub(crate) fn spec_for(verb: &str) -> Option<FlagSpec> {
    find_group(verb).map(|group| group.spec(verb))
}

fn find_group(verb: &str) -> Option<&'static dyn ReadGroup> {
    groups()
        .into_iter()
        .find(|group| group.verbs().contains(&verb))
}

/// Add the in-band `truncated` marker to a success payload that reports
/// `total`/`offset`/`items` (used by `inspect`).
pub(crate) fn add_truncated(payload: &mut Value) {
    let count = |key: &str| {
        payload
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
    };
    let offset = count("offset").unwrap_or(0);
    let total = count("total").unwrap_or(0);
    let returned = payload
        .get("items")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    payload["truncated"] = Value::Bool(offset.saturating_add(returned) < total);
}

/// Mark a page truncated when it filled the resolved `limit`.
pub(crate) fn mark_truncated(payload: &mut Value, key: &str) {
    let returned = payload
        .get(key)
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let limit = payload
        .get("limit")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok());
    payload["truncated"] = Value::Bool(limit.is_some_and(|limit| returned >= limit));
}

pub(crate) fn emit(io: &mut CliIo, mode: OutputMode, payload: &Value, text: &str) {
    let _ = match mode {
        OutputMode::Json => cli_output::write_json(&mut io.out, payload),
        OutputMode::Text => cli_output::write_text(&mut io.out, text),
    };
}

pub(crate) fn fail(io: &mut CliIo, mode: OutputMode, error: &CliError) -> ExitCode {
    let color = cli_output::supports_color(io.is_tty, io.no_color);
    let _ = cli_output::write_error(&mut io.err, error, mode, color);
    error.exit_code()
}

// A parse error's mode is resolved from the raw tokens (`mode_from_raw`), so an
// explicit `--output` still wins even though the arguments failed to parse.
// Subcommand help is rendered here, before any database is opened, so
// `clio <verb> --help` never touches the store.
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

pub(crate) fn call_tool(
    state: &Arc<McpState>,
    tool: &str,
    arguments: &Value,
) -> Result<Value, CliError> {
    let handler = clio_mcp::protocol::McpHandler::new(Arc::clone(state));
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": tool, "arguments": arguments }
    });
    let Some(response) = handler.handle_message(&request) else {
        return Err(CliError::operational(
            "internal",
            "no response from in-process dispatcher",
        ));
    };
    if let Some(error) = response.get("error") {
        let code = match error.get("code") {
            Some(code) => code.to_string(),
            None => "internal".to_owned(),
        };
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("dispatcher error");
        return Err(tool_failure(&code, message));
    }
    let Some(structured) = response.pointer("/result/structuredContent").cloned() else {
        return Err(CliError::operational(
            "internal",
            "tool result missing structuredContent",
        ));
    };
    if structured.get("ok").and_then(Value::as_bool) == Some(true) {
        return Ok(structured);
    }
    let code = structured
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("internal");
    let message = structured
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("tool call failed");
    Err(tool_failure(code, message))
}

/// Build the CLI error for an in-process tool failure, adding the cross-process
/// key-provider hint when the failure is a missing DEK.
fn tool_failure(code: &str, message: &str) -> CliError {
    let error = CliError::tool_failure(code, message);
    match crate::cli_error::cross_process_dek_hint(message) {
        Some(hint) => error.with_hint(hint),
        None => error,
    }
}

#[cfg(test)]
#[path = "cli_read_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "cli_read_help_tests.rs"]
mod help_tests;

#[cfg(test)]
#[path = "cli_read_group_tests.rs"]
mod group_tests;

#[cfg(test)]
#[path = "cli_read_output_tests.rs"]
mod cli_read_output_tests;
