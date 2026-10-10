// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `clio install` and `clio uninstall`
//! command surfaces: flag parsing, help, harness selection, executable-path
//! resolution, delegate-outcome mapping, and exit codes.
//!
//! ## Owns
//! - The `clio install|uninstall` entry points and their flags (`--harness`)
//! - The parse-and-decide step both verbs share, so they cannot drift
//! - The `clio` executable lookup, with an injected seam for tests
//! - The direct-write path: resolving the harness's config file through the
//!   injectable `DirectWriteEnv` seam and mapping the write outcome to
//!   messages and exit codes
//! - The mapping from delegate outcomes to messages and exit codes
//! - The surface help, including the supported-harness list
//! - The success text, including a harness's config-surface note naming the
//!   file that received the entry
//!
//! ## Does not own
//! - Running any harness command or writing any harness config; the delegate
//!   module owns every spawned process, and the writer modules own the file
//!   merge, backup, and parse-refusal primitives
//! - Which harnesses exist, their names, and their argv shapes
//!   (`agent_harness`)
//!
//! ## Boundary
//!
//! Changes to these commands' flags, help, or exit-code mapping should
//! normally require changes here. Changes to how a harness registration is
//! performed should not require changes here.
//!
//! ## Exit codes
//!
//! - `0`: success; the delegate reported the change.
//! - `1`: a precondition failed (the clio executable could not be resolved, the
//!   harness binary is missing, the harness command exited non-zero, the
//!   change could not be verified, or the harness's config file could not be
//!   read, parsed, or written).
//! - `2`: unknown or missing `--harness`, or any other flag error.
//!
//! Keep this module focused on the install and uninstall command surfaces.

use crate::agent_harness::{self, HarnessRecord, Mechanism, SERVER_NAME};
use crate::agent_harness_delegate::{self, DelegateEnv, DelegateError};
use crate::agent_harness_targets::{DirectWriteConfig, DirectWriteEnv};
use crate::agent_harness_write::{RemoveOutcome, WriteError};
use crate::agent_harness_write_install::{
    InstallOutcome, install_server_from_file, remove_server_from_direct_write,
};
use crate::cli_error::CliError;
use crate::cli_top_level::{parse_flags, report_surface_usage_error, wants_help};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The flags both verbs accept. Any other flag is a usage error rather than a
/// silently ignored token.
const HARNESS_FLAGS: &[&str] = &["harness"];

/// The two verbs, so the parse-decide-report flow runs once for both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    Install,
    Uninstall,
}

impl Verb {
    /// The command name a user sees.
    fn label(self) -> &'static str {
        match self {
            Verb::Install => "install",
            Verb::Uninstall => "uninstall",
        }
    }

    /// What the verb does, for the help text.
    fn purpose(self) -> &'static str {
        match self {
            Verb::Install => "Register the clio MCP server with a supported agent harness.",
            Verb::Uninstall => {
                "Remove the clio MCP server registration from a supported agent harness."
            }
        }
    }
}

/// What one verb should do after parsing.
#[derive(Debug)]
enum Decision {
    Help,
    Run(&'static HarnessRecord),
}

/// `clio install`: register the clio MCP server with the named harness.
pub fn install(args: &[String]) -> ExitCode {
    run(Verb::Install, args)
}

/// `clio uninstall`: remove the clio MCP server registration.
pub fn uninstall(args: &[String]) -> ExitCode {
    run(Verb::Uninstall, args)
}

/// One flow for both verbs: parse, decide, resolve the executable (install
/// only), then run the record's mechanism — the delegate for vendor-CLI
/// harnesses, the direct writer for direct-write harnesses — and map the
/// outcome to an exit code. The production delegate calls and environment
/// are built here; the delegate owns every spawned process.
fn run(verb: Verb, args: &[String]) -> ExitCode {
    let install = |record: &HarnessRecord, exe: &Path| -> Result<String, DelegateError> {
        agent_harness_delegate::install(record, SERVER_NAME, exe, &DelegateEnv::production())
    };
    let uninstall = |record: &HarnessRecord| -> Result<String, DelegateError> {
        agent_harness_delegate::uninstall(record, SERVER_NAME, &DelegateEnv::production())
    };
    run_with(
        verb,
        args,
        &current_exe,
        &|| DirectWriteEnv::production(),
        &install,
        &uninstall,
    )
}

/// [`run`] with the executable lookup, the direct-write environment, and both
/// delegate calls injected, so a test can decide outcomes without spawning a
/// process or touching the host environment (the `mcp_options_with` pattern).
fn run_with(
    verb: Verb,
    args: &[String],
    exe: &dyn Fn() -> std::io::Result<PathBuf>,
    direct_write_env: &dyn Fn() -> DirectWriteEnv,
    install_call: &dyn Fn(&HarnessRecord, &Path) -> Result<String, DelegateError>,
    uninstall_call: &dyn Fn(&HarnessRecord) -> Result<String, DelegateError>,
) -> ExitCode {
    let decision = match decide(args) {
        Ok(decision) => decision,
        Err(error) => return report_surface_usage_error(&error, args),
    };
    match decision {
        Decision::Help => {
            print_help(verb);
            ExitCode::SUCCESS
        }
        Decision::Run(record) => match record.mechanism {
            Mechanism::DirectWrite => run_direct_write(verb, record, exe, direct_write_env),
            Mechanism::Delegate => match verb {
                Verb::Install => {
                    let path = match exe() {
                        Ok(path) => path,
                        Err(error) => return fail(verb, &exe_failure_message(&error)),
                    };
                    report(verb, record, install_call(record, &path))
                }
                Verb::Uninstall => report(verb, record, uninstall_call(record)),
            },
        },
    }
}

/// The direct-write path: resolve the harness's config file for this machine,
/// build the server argv from the resolved executable, and map the write
/// outcome to a message and exit code. Install needs the executable, because
/// the written entry and the refusal snippet name it. Uninstall removes by
/// name, so a failed executable lookup is not fatal: the lookup only feeds
/// the refusal snippet, and uninstall proceeds without it rather than failing
/// an otherwise valid removal.
fn run_direct_write(
    verb: Verb,
    record: &HarnessRecord,
    exe: &dyn Fn() -> std::io::Result<PathBuf>,
    direct_write_env: &dyn Fn() -> DirectWriteEnv,
) -> ExitCode {
    let Some(config) = record.direct_write_config(&direct_write_env()) else {
        return fail(
            verb,
            &format!("cannot resolve the config file for `{}`", record.name),
        );
    };
    let argv: Vec<OsString> = match verb {
        Verb::Install => match exe() {
            Ok(path) => agent_harness::server_argv(&path),
            Err(error) => return fail(verb, &exe_failure_message(&error)),
        },
        // Uninstall removes by name, so the executable feeds only the manual
        // refusal snippet. When it cannot be resolved, uninstall still runs.
        Verb::Uninstall => exe()
            .map(|path| agent_harness::server_argv(&path))
            .unwrap_or_default(),
    };
    match direct_write_outcome_text(verb, record, &config, &argv) {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(message) => fail(verb, &message),
    }
}

/// Perform the direct-write install or uninstall and render its message: the
/// success text on `Ok`, the plain failure text on `Err`. The message is
/// returned rather than printed, so a test can check it and the file effects
/// together without capturing stdout.
fn direct_write_outcome_text(
    verb: Verb,
    record: &HarnessRecord,
    config: &DirectWriteConfig,
    argv: &[OsString],
) -> Result<String, String> {
    match verb {
        Verb::Install => match install_server_from_file(config, SERVER_NAME, argv) {
            Ok(outcome) => Ok(direct_write_install_text(record, config, &outcome)),
            Err(error) => Err(write_error_message(&error)),
        },
        Verb::Uninstall => match remove_server_from_direct_write(config, SERVER_NAME, argv) {
            Ok(outcome) => Ok(direct_write_uninstall_text(config, &outcome)),
            Err(error) => Err(write_error_message(&error)),
        },
    }
}

/// The install success text: the target path, whether the file was created
/// or merged, and the backup when one was made.
fn direct_write_install_text(
    record: &HarnessRecord,
    config: &DirectWriteConfig,
    outcome: &InstallOutcome,
) -> String {
    match outcome {
        InstallOutcome::Created => format!(
            "created {} and registered clio with {}",
            config.path.display(),
            record.name
        ),
        InstallOutcome::Merged { backup } => format!(
            "registered clio with {} in {}; the previous file was backed up to {}",
            record.name,
            config.path.display(),
            backup.display()
        ),
    }
}

/// The uninstall success text: what was removed with its backup, or the
/// clear no-op when Clio's entry was not there.
fn direct_write_uninstall_text(config: &DirectWriteConfig, outcome: &RemoveOutcome) -> String {
    match outcome {
        RemoveOutcome::Removed { backup } => format!(
            "removed clio from {}; the previous file was backed up to {}",
            config.path.display(),
            backup.display()
        ),
        RemoveOutcome::Absent => format!(
            "no clio entry in {}; nothing to remove",
            config.path.display()
        ),
    }
}

/// The plain message for a direct-write failure. An unparseable file was not
/// changed, so the message carries the manual snippet the user can paste by
/// hand; the writer already combined the entry snippet with the offending
/// line's number.
fn write_error_message(error: &WriteError) -> String {
    match error {
        WriteError::Unparseable { path, snippet } => format!(
            "{} could not be parsed as JSON, so it was not changed; add this entry by hand:\n{}",
            path.display(),
            snippet
        ),
        WriteError::Io { detail } => detail.clone(),
    }
}

/// The executable-path lookup production uses.
fn current_exe() -> std::io::Result<PathBuf> {
    std::env::current_exe()
}

/// Parse the arguments and decide what the verb should do. Pure: no lookup, no
/// delegate call, no output.
fn decide(args: &[String]) -> Result<Decision, CliError> {
    if wants_help(args) {
        return Ok(Decision::Help);
    }
    let flags = parse_flags(args.iter().cloned(), HARNESS_FLAGS)?;
    if flags.contains_key("help") {
        return Ok(Decision::Help);
    }
    let name = match flags.get("harness") {
        Some(name) => name.as_str(),
        None => return Err(missing_harness()),
    };
    match agent_harness::resolve(name) {
        Some(record) => Ok(Decision::Run(record)),
        None => match agent_harness::normalize(name) {
            Some(canonical) => Err(known_but_unsupported(canonical)),
            None => Err(unknown_harness(name)),
        },
    }
}

/// The supported-harness list, generated from the registry so the help and the
/// usage errors cannot drift apart.
fn supported_hint() -> String {
    format!(
        "supported harnesses: {}",
        agent_harness::supported_names().join(", ")
    )
}

/// A call with no `--harness` flag.
fn missing_harness() -> CliError {
    CliError::usage("--harness is required").with_hint(supported_hint())
}

/// A call naming a harness the registry does not know.
fn unknown_harness(name: &str) -> CliError {
    CliError::usage(format!("unknown harness `{name}`")).with_hint(supported_hint())
}

/// A call naming a harness the registry recognizes by name but does not yet
/// implement, such as `cursor` or the Copilot alias `vs code`. The name is
/// understood, so the refusal says so rather than calling it unknown.
fn known_but_unsupported(canonical: &str) -> CliError {
    CliError::usage(format!(
        "harness `{canonical}` is known but not yet supported"
    ))
    .with_hint(supported_hint())
}

/// Why the executable lookup failed, for a plain failure message.
fn exe_failure_message(error: &std::io::Error) -> String {
    format!("cannot resolve the clio executable: {error}")
}

/// The plain message for a delegate failure. `BinaryMissing` names the missing
/// binary and the manual command, so the caller can act without Clio.
fn failure_message(error: &DelegateError) -> String {
    match error {
        DelegateError::BinaryMissing {
            binary,
            manual_snippet,
        } => format!(
            "the `{binary}` command was not found; install it first, or run: {manual_snippet}"
        ),
        DelegateError::CommandFailed { exit_code, output } => {
            format!("the harness command exited with status {exit_code}: {output}")
        }
        DelegateError::VerificationFailed { detail } => {
            format!("could not verify the change: {detail}")
        }
    }
}

/// The config-surface note for a harness whose entry lands in a vendor-owned
/// file the user may not expect. The delegate writes the Antigravity CLI
/// global config, so the note names that file. Only Antigravity has a note
/// today, so no other harness gets one.
fn config_surface_note(record: &HarnessRecord) -> Option<String> {
    (record.name == "antigravity").then(|| {
        format!(
            "note: antigravity writes the CLI global config (~/{})",
            record.config_file
        )
    })
}

/// The text a successful install or uninstall prints for this harness: the
/// delegate's message, with the config-surface note appended when the harness
/// has one. Pure, so a test can check the note without capturing stdout.
fn success_text(record: &HarnessRecord, message: &str) -> String {
    match config_surface_note(record) {
        Some(note) => format!("{message}\n{note}"),
        None => message.to_owned(),
    }
}

/// Map a delegate outcome: the message on stdout and exit 0, or a plain
/// failure message on stderr and exit 1.
fn report(verb: Verb, record: &HarnessRecord, outcome: Result<String, DelegateError>) -> ExitCode {
    match outcome {
        Ok(message) => {
            println!("{}", success_text(record, &message));
            ExitCode::SUCCESS
        }
        Err(error) => fail(verb, &failure_message(&error)),
    }
}

/// One CLI failure shape: a clear message on stderr, exit 1.
fn fail(verb: Verb, message: &str) -> ExitCode {
    eprintln!("clio {}: {message}", verb.label());
    ExitCode::from(1)
}

/// The surface help: the usage line, what the verb does, the supported list
/// from the registry, and one example.
fn print_help(verb: Verb) {
    println!("{}", help_text(verb));
}

/// The full help text, split from `print_help` so a test can read it without
/// capturing stdout. The Antigravity config-surface note is included because
/// help lists every supported harness, and an Antigravity user needs to know
/// which file received the entry.
fn help_text(verb: Verb) -> String {
    let usage = crate::cli_help_usage::usage_line(verb.label()).unwrap_or(match verb {
        Verb::Install => "clio install --harness <name>",
        Verb::Uninstall => "clio uninstall --harness <name>",
    });
    let mut text = format!(
        "{usage}\n\n{}\n\n{}\n\nExample: clio {} --harness claude",
        verb.purpose(),
        supported_hint(),
        verb.label(),
    );
    if let Some(note) = agent_harness::resolve("antigravity").and_then(config_surface_note) {
        text.push_str("\n\n");
        text.push_str(&note);
    }
    text
}

#[cfg(test)]
#[path = "install_cli_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "install_cli_direct_write_tests.rs"]
mod direct_write_tests;
