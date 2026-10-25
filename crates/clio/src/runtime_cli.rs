// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared `clio` engine that runs a
//! `clio_config::Runtime` tool from a two-token group command (`config …`,
//! `ranking …`): subcommand resolution, flag parsing, `--help`, and the
//! in-process `Runtime::call_tool` bridge so the CLI and any future binding
//! share one implementation.
//!
//! ## Owns
//! - The `RuntimeGroup` contract (verbs, flag spec, call builder, text renderer)
//! - Subcommand resolution, `--help`, output-mode selection, and error mapping
//!   for the runtime-backed commands
//!
//! ## Does not own
//! - Any concrete command group's verbs, flags, or views (see `config_cli` /
//!   `ranking_cli`)
//! - Config/ranking semantics (`clio-config` owns those)
//! - The output/error contracts (see `cli_output` / `cli_error`)
//!
//! ## Boundary
//!
//! Changes to how the engine talks to a runtime command group should normally
//! require changes here. Changes to a specific group should not.
//!
//! Keep this module focused on the runtime-backed command engine.

use crate::cli_args::{self, FlagSpec, ParsedArgs};
use crate::cli_error::{CliError, EXIT_OK};
use crate::cli_help;
use crate::cli_output;
use crate::cli_read::{CliIo, emit, fail};
use clio::Runtime;
use serde_json::Value;
use std::process::ExitCode;

/// One disjoint command group backed by `clio_config::Runtime` tool dispatch.
///
/// The engine resolves a canonical two-token verb (for example `config get`),
/// then asks the owning group for its flag spec, tool call, and text view.
pub(crate) trait RuntimeGroup: Sync {
    /// Canonical verb keys this group owns (for example `["config get"]`).
    fn verbs(&self) -> &'static [&'static str];

    /// The flags `verb` accepts, globals included.
    fn spec(&self, verb: &str) -> FlagSpec;

    /// Decode `verb` into its `Runtime` tool name and argument object.
    fn build(&self, verb: &str, parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError>;

    /// Side effects the verb must complete before its payload is reported,
    /// called once the tool call succeeded and before rendering. The default
    /// verb has none.
    ///
    /// # Errors
    /// The side effect could not be completed, so the payload must not be
    /// reported as a success.
    fn commit(&self, verb: &str, payload: &Value, runtime: &mut Runtime) -> Result<(), CliError> {
        let _ = (verb, payload, runtime);
        Ok(())
    }

    /// Render a success payload as deterministic human text.
    fn render(&self, verb: &str, payload: &Value) -> String;
}

/// Resolve, parse, and run one runtime-backed command group.
pub(crate) fn run(
    group: &dyn RuntimeGroup,
    cmd: &str,
    args: Vec<String>,
    io: &mut CliIo,
    runtime: &mut Runtime,
) -> ExitCode {
    let fallback = cli_output::mode_from_raw(&args, io.is_tty);
    let mut tokens = args.into_iter();
    let Some(sub) = tokens.next() else {
        return fail(io, fallback, &missing_subcommand(cmd, group));
    };
    if sub == "--help" || sub == "-h" {
        let _ = cli_output::write_text(&mut io.out, &group_help(cmd));
        return ExitCode::from(EXIT_OK);
    }
    let verb = format!("{cmd} {sub}");
    if !group.verbs().contains(&verb.as_str()) {
        return fail(io, fallback, &unknown_subcommand(cmd, &sub, group));
    }
    let parsed = match cli_args::parse(&group.spec(&verb), tokens) {
        Ok(parsed) => parsed,
        Err(error) => return fail(io, fallback, &error),
    };
    if let Some(code) = help_if_requested(&verb, &parsed, io) {
        return code;
    }
    let mode = match cli_output::resolve_output(parsed.value("output"), io.is_tty) {
        Ok(mode) => mode,
        Err(error) => return fail(io, fallback, &error),
    };
    let (tool, tool_args) = match group.build(&verb, &parsed) {
        Ok(call) => call,
        Err(error) => return fail(io, mode, &error),
    };
    let response = runtime.call_tool(tool, &tool_args);
    if response.get("ok").and_then(Value::as_bool) == Some(true) {
        if let Err(error) = group.commit(&verb, &response, runtime) {
            return fail(io, mode, &error);
        }
        let text = group.render(&verb, &response);
        emit(io, mode, &response, &text);
        ExitCode::from(EXIT_OK)
    } else {
        fail(io, mode, &envelope_error(&response))
    }
}

/// The group's `--help` text: its subcommands and the tool each binds.
fn group_help(cmd: &str) -> String {
    let subs = cli_help::group_subcommands(cmd).unwrap_or(&[]);
    cli_help::group_help(cmd, subs)
}

/// The error for a missing subcommand, naming the first one.
fn missing_subcommand(cmd: &str, group: &dyn RuntimeGroup) -> CliError {
    let first = group
        .verbs()
        .first()
        .and_then(|verb| verb.strip_prefix(&format!("{cmd} ")))
        .unwrap_or_default();
    CliError::usage_hint(
        format!("`{cmd}` requires a subcommand"),
        format!("try `clio {cmd} {first}`"),
    )
}

/// The error for an unknown subcommand, suggesting the closest verb.
fn unknown_subcommand(cmd: &str, sub: &str, group: &dyn RuntimeGroup) -> CliError {
    let subs: Vec<&str> = group
        .verbs()
        .iter()
        .filter_map(|verb| verb.strip_prefix(&format!("{cmd} ")))
        .collect();
    let hint = cli_args::suggest(sub, &subs).map_or_else(
        || {
            format!(
                "try `clio {cmd} {}`",
                subs.first().copied().unwrap_or_default()
            )
        },
        |best| format!("did you mean `clio {cmd} {best}`?"),
    );
    CliError::usage(format!("unknown `{cmd}` subcommand `{sub}`")).with_hint(hint)
}

/// Render a verb's `--help`, before any tool call.
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

/// Map a failed `Runtime::call_tool` envelope into the CLI error contract.
fn envelope_error(response: &Value) -> CliError {
    let code = response
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("internal");
    let message = response
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("tool call failed");
    CliError::tool_failure(code, message)
}

/// A rejected positional list for a verb that takes none.
pub(crate) fn no_positionals(parsed: &ParsedArgs) -> Result<(), CliError> {
    match parsed.positionals().first() {
        Some(extra) => Err(CliError::usage(format!(
            "unexpected extra argument `{extra}`"
        ))),
        None => Ok(()),
    }
}

/// Pretty-print a payload with the tool envelope's `ok` field removed.
pub(crate) fn pretty_without_ok(payload: &Value) -> String {
    let mut body = payload.clone();
    if let Some(object) = body.as_object_mut() {
        object.remove("ok");
    }
    serde_json::to_string_pretty(&body).expect("runtime payload serializes")
}

#[cfg(test)]
#[path = "runtime_cli_tests.rs"]
pub(crate) mod tests;
