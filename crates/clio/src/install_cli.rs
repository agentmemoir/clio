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
//! - The `clio install|uninstall` entry points and the flow after parsing: the
//!   skill step, the executable lookup, the delegate or direct-write call, and
//!   the exit code
//! - The install flow's order: write the embedded skill assets, then register;
//!   with no `--harness`, write the portable package only and skip registration
//! - Threading `--force` from the parsed decision into the skill step, so a
//!   conflicting skill package is replaced instead of refused
//! - The `clio` executable lookup, with an injected seam for tests
//! - The direct-write path: resolving the harness's config file through the
//!   injectable `DirectWriteEnv` seam and mapping the write outcome to
//!   messages and exit codes
//! - The mapping from delegate outcomes to messages and exit codes
//! - The success text, including a harness's config-surface note naming the
//!   file that received the entry (see `install_cli_help`)
//!
//! ## Does not own
//! - Parsing the arguments or deciding which harness to run (see
//!   `install_cli_decide`)
//! - Running any harness command or writing any harness config; the delegate
//!   module owns every spawned process, and the writer modules own the file
//!   merge, backup, and parse-refusal primitives
//! - Writing the skill package (`skill_install`) or its text
//!   (`skill_install_text`)
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
//!   change could not be verified, the harness's config file could not be
//!   read, parsed, or written, or the portable skill write failed).
//! - `2`: an unknown `--harness`, a missing `--harness` on uninstall, or any
//!   other flag error.
//!
//! Keep this module focused on the install and uninstall command surfaces.

use crate::agent_harness::{self, HarnessRecord, Mechanism, SERVER_NAME};
use crate::agent_harness_delegate::{self, DelegateEnv, DelegateError};
use crate::agent_harness_targets::{DirectWriteConfig, DirectWriteEnv};
use crate::agent_harness_write::{RemoveOutcome, WriteError};
use crate::agent_harness_write_install::{
    InstallOutcome, install_server_from_file, remove_server_from_direct_write,
};
use crate::cli_top_level::report_surface_usage_error;
use crate::install_cli_decide::{Decision, Verb, decide};
use crate::install_cli_help::{print_help, success_text, supported_hint};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// `clio install`: register the clio MCP server with the named harness, or
/// write the portable skill package when no harness is named. `--force`
/// replaces a conflicting skill package instead of refusing it.
pub fn install(args: &[String]) -> ExitCode {
    run(Verb::Install, args)
}

/// `clio uninstall`: remove the clio MCP server registration.
pub fn uninstall(args: &[String]) -> ExitCode {
    run(Verb::Uninstall, args)
}

/// One flow for both verbs: parse, decide, resolve the executable (install only), then run the record's mechanism — the delegate for vendor-CLI harnesses, the direct writer for direct-write harnesses — and map the outcome to an exit code. The production delegate calls and environment are built here; the delegate owns every spawned process.
fn run(verb: Verb, args: &[String]) -> ExitCode {
    let install = |record: &HarnessRecord, exe: &Path| -> Result<String, DelegateError> {
        agent_harness_delegate::install(record, SERVER_NAME, exe, &DelegateEnv::production())
    };
    let uninstall = |record: &HarnessRecord| -> Result<String, DelegateError> {
        agent_harness_delegate::uninstall(record, SERVER_NAME, &DelegateEnv::production())
    };
    run_with_skills(
        verb,
        args,
        &current_exe,
        &|| DirectWriteEnv::production(),
        &install,
        &uninstall,
        &|record, skill_env, force| match record {
            Some(record) => crate::skill_install::install_and_report(record, skill_env, force),
            None => crate::skill_install::install_portable_and_report(skill_env, force),
        },
    )
}

/// The skill step: write the embedded skill assets before registration. `None`
/// means `clio install` named no harness and only the portable package is
/// written; `force` replaces the assets clio recorded instead of refusing a
/// conflict.
pub(crate) type SkillStep<'a> =
    &'a dyn Fn(Option<&HarnessRecord>, &DirectWriteEnv, bool) -> Result<(), String>;

/// [`run_with_skills`] with the skill step left out. Test-only: a test that injects the executable lookup and both delegate calls must never write into a real home directory.
#[cfg(test)]
fn run_with(
    verb: Verb,
    args: &[String],
    exe: &dyn Fn() -> std::io::Result<PathBuf>,
    direct_write_env: &dyn Fn() -> DirectWriteEnv,
    install_call: &dyn Fn(&HarnessRecord, &Path) -> Result<String, DelegateError>,
    uninstall_call: &dyn Fn(&HarnessRecord) -> Result<String, DelegateError>,
) -> ExitCode {
    let skill_step = |_: Option<&HarnessRecord>, _: &DirectWriteEnv, _: bool| Ok(());
    run_with_skills(
        verb,
        args,
        exe,
        direct_write_env,
        install_call,
        uninstall_call,
        &skill_step,
    )
}

/// [`run`] with the executable lookup, the direct-write environment, both delegate calls, and the skill step injected, so a test can decide outcomes without spawning a process or touching the host environment (the `mcp_options_with` pattern).
pub(crate) fn run_with_skills(
    verb: Verb,
    args: &[String],
    exe: &dyn Fn() -> std::io::Result<PathBuf>,
    direct_write_env: &dyn Fn() -> DirectWriteEnv,
    install_call: &dyn Fn(&HarnessRecord, &Path) -> Result<String, DelegateError>,
    uninstall_call: &dyn Fn(&HarnessRecord) -> Result<String, DelegateError>,
    skill_step: SkillStep<'_>,
) -> ExitCode {
    let decision = match decide(verb, args) {
        Ok(decision) => decision,
        Err(error) => return report_surface_usage_error(&error, args),
    };
    match decision {
        Decision::Help => {
            print_help(verb);
            ExitCode::SUCCESS
        }
        Decision::Portable { force } => {
            if let Err(message) = skill_step(None, &direct_write_env(), force) {
                return fail(verb, &message);
            }
            println!("{}", supported_hint());
            ExitCode::SUCCESS
        }
        Decision::Run { record, force } => {
            if verb == Verb::Install
                && let Err(message) = skill_step(Some(record), &direct_write_env(), force)
            {
                return fail(verb, &message);
            }
            match record.mechanism {
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
            }
        }
    }
}

/// The direct-write path: resolve the harness's config file for this machine, build the server argv from the resolved executable, and map the write outcome to a message and exit code. Install needs the executable, because the written entry and the refusal snippet name it. Uninstall removes by name, so a failed executable lookup is not fatal: the lookup only feeds the refusal snippet, and uninstall proceeds without it rather than failing an otherwise valid removal.
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

/// Perform the direct-write install or uninstall and render its message: the success text on `Ok`, the plain failure text on `Err`. The message is returned rather than printed, so a test can check it and the file effects together without capturing stdout.
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

/// The plain message for a direct-write failure. An unparseable file was not changed, so the message carries the manual snippet the user can paste by hand; the writer already combined the entry snippet with the offending line's number.
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

#[cfg(test)]
#[path = "install_cli_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "install_cli_portable_tests.rs"]
mod portable_tests;

#[cfg(test)]
#[path = "install_cli_direct_write_tests.rs"]
mod direct_write_tests;
