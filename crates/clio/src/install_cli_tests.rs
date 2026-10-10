// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio install` / `clio uninstall` surfaces: the shared
//! parse-and-decide step, delegate-outcome-to-exit-code mapping, and the help
//! text. Every test injects the executable lookup and both delegate calls; no
//! test spawns a process, reads the filesystem, or touches the host
//! environment.
//!
//! ## Owns
//! - Assertions over `decide`, `run_with`, `failure_message`,
//!   `exe_failure_message`, `success_text`, and `help_text`, with delegate
//!   doubles that must never run
//!
//! ## Does not own
//! - The surface implementation under test (`install_cli`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::agent_harness_targets::DirectWriteEnv;
use std::cell::RefCell;
use std::path::Path;
use std::path::PathBuf;

/// Shared with `direct_write_tests`, which tests the other mechanism of the
/// same surface.
pub(super) fn never_install(_: &HarnessRecord, _: &Path) -> Result<String, DelegateError> {
    panic!("the install delegate must not run");
}

pub(super) fn never_uninstall(_: &HarnessRecord) -> Result<String, DelegateError> {
    panic!("the uninstall delegate must not run");
}

pub(super) fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(std::string::ToString::to_string).collect()
}

/// A synthetic executable path for tests that need the lookup to succeed.
pub(super) fn fake_exe() -> PathBuf {
    PathBuf::from("clio")
}

/// A direct-write environment a delegate-harness test must never consult:
/// consulting it fails the test, so passing this proves the delegate path
/// ran instead.
fn never_direct_write_env() -> DirectWriteEnv {
    panic!("a delegate record must not consult the direct-write environment");
}

#[test]
fn help_is_answered_before_any_lookup_or_delegate_call() {
    let code = run_with(
        Verb::Install,
        &args(&["--help"]),
        &|| panic!("help must not resolve the executable"),
        &never_direct_write_env,
        &never_install,
        &never_uninstall,
    );
    assert_eq!(code, ExitCode::SUCCESS);
}

#[test]
fn uninstall_also_answers_help() {
    let code = run_with(
        Verb::Uninstall,
        &args(&["-h"]),
        &|| panic!("help must not resolve the executable"),
        &never_direct_write_env,
        &never_install,
        &never_uninstall,
    );
    assert_eq!(code, ExitCode::SUCCESS);
}

#[test]
fn help_with_an_explicit_value_is_answered_through_parse_flags() {
    // `--help=x` is not one of the exact tokens `wants_help` matches, so it
    // must reach the help decision through the parsed `help` flag instead.
    let decision = decide(&args(&["--help=x"])).expect("--help=x is a valid help spelling");
    assert!(matches!(decision, Decision::Help), "{decision:?}");
}

#[test]
fn missing_harness_is_a_usage_error_before_the_delegate_runs() {
    let code = run_with(
        Verb::Install,
        &args(&[]),
        &|| Ok(fake_exe()),
        &never_direct_write_env,
        &never_install,
        &never_uninstall,
    );
    assert_eq!(
        code,
        ExitCode::from(2),
        "a missing --harness must fail with the usage code"
    );
}

#[test]
fn missing_harness_usage_error_lists_all_five_supported_harnesses() {
    let error = decide(&args(&[])).expect_err("a missing --harness must be refused");
    assert_eq!(error.exit_code(), ExitCode::from(2));
    let hint = error.hint().unwrap_or_default();
    assert!(
        hint.contains(
            "supported harnesses: claude, codex, opencode, antigravity, hermes, cursor, github copilot"
        ),
        "the hint must name the exact registry list: {hint:?}"
    );
}

#[test]
fn unknown_harness_is_refused_with_the_supported_list() {
    let error = decide(&args(&["--harness", "vim"])).expect_err("vim is not supported");
    assert_eq!(error.exit_code(), ExitCode::from(2));
    assert!(
        error.message().contains("unknown harness `vim`"),
        "{error:?}"
    );
    assert!(error.hint().unwrap_or_default().contains("claude"));
}

#[test]
fn gh_copilot_is_an_unknown_harness_usage_error_with_the_supported_list() {
    let error = decide(&args(&["--harness", "gh-copilot"]))
        .expect_err("gh-copilot is not an accepted name");
    assert_eq!(error.exit_code(), ExitCode::from(2));
    assert!(
        error.message().contains("unknown harness `gh-copilot`"),
        "{error:?}"
    );
    assert!(
        error
            .hint()
            .unwrap_or_default()
            .contains("supported harnesses: claude, codex, opencode, antigravity, hermes, cursor, github copilot"),
        "{error:?}"
    );
}

#[test]
fn cursor_resolves_to_an_installable_direct_write_record() {
    let decision = decide(&args(&["--harness", "cursor"])).expect("cursor is supported");
    assert!(
        matches!(decision, Decision::Run(record) if record.mechanism == Mechanism::DirectWrite),
        "{decision:?}"
    );
}

#[test]
fn vs_code_alias_resolves_to_the_github_copilot_direct_write_record() {
    let decision = decide(&args(&["--harness", "vs code"])).expect("the vs code alias resolves");
    assert!(
        matches!(decision, Decision::Run(record) if record.name == "github copilot"
            && record.mechanism == Mechanism::DirectWrite),
        "{decision:?}"
    );
}

#[test]
fn github_copilot_resolves_to_an_installable_direct_write_record() {
    let decision =
        decide(&args(&["--harness", "github copilot"])).expect("github copilot is supported");
    assert!(
        matches!(decision, Decision::Run(record) if record.mechanism == Mechanism::DirectWrite),
        "{decision:?}"
    );
}

#[test]
fn unknown_flag_is_a_usage_error_before_the_delegate_runs() {
    let code = run_with(
        Verb::Install,
        &args(&["--bogus"]),
        &|| panic!("an unknown flag must fail before the executable lookup"),
        &never_direct_write_env,
        &never_install,
        &never_uninstall,
    );
    assert_eq!(
        code,
        ExitCode::from(2),
        "an unknown flag must fail with the usage code"
    );
}

#[test]
fn harness_with_no_value_is_a_usage_error_before_the_delegate_runs() {
    // `--harness` with no value becomes the value-less marker `true`, which the
    // registry does not know, so the surface refuses it as a usage error.
    let code = run_with(
        Verb::Install,
        &args(&["--harness"]),
        &|| panic!("a missing flag value must fail before the executable lookup"),
        &never_direct_write_env,
        &never_install,
        &never_uninstall,
    );
    assert_eq!(
        code,
        ExitCode::from(2),
        "a value-less --harness must fail with the usage code"
    );
}

#[test]
fn canonical_name_resolves_to_its_record() {
    let decision = decide(&args(&["--harness", "claude"])).expect("claude is supported");
    assert!(matches!(decision, Decision::Run(record) if record.name == "claude"));
}

#[test]
fn alias_and_capitalization_resolve_to_the_canonical_record() {
    let decision = decide(&args(&["--harness", "Claude-Code"])).expect("alias must resolve");
    assert!(matches!(decision, Decision::Run(record) if record.name == "claude"));
}

#[test]
fn install_success_passes_the_record_and_exe_to_the_delegate_and_exits_0() {
    let calls = RefCell::new(Vec::new());
    let install = |record: &HarnessRecord, exe: &Path| -> Result<String, DelegateError> {
        calls
            .borrow_mut()
            .push(format!("{} {}", record.name, exe.display()));
        Ok("registered clio with claude".to_owned())
    };
    let code = run_with(
        Verb::Install,
        &args(&["--harness", "claude"]),
        &|| Ok(fake_exe()),
        &never_direct_write_env,
        &install,
        &never_uninstall,
    );
    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(*calls.borrow(), vec!["claude clio".to_owned()]);
}

#[test]
fn a_failing_exe_lookup_fails_before_the_delegate_runs() {
    let code = run_with(
        Verb::Install,
        &args(&["--harness", "claude"]),
        &|| -> std::io::Result<PathBuf> {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"))
        },
        &never_direct_write_env,
        &never_install,
        &never_uninstall,
    );
    assert_eq!(
        code,
        ExitCode::from(1),
        "an exe-lookup failure is a precondition failure, not a usage error"
    );
}

#[test]
fn binary_missing_maps_to_exit_1() {
    let install = |_: &HarnessRecord, _: &Path| -> Result<String, DelegateError> {
        Err(DelegateError::BinaryMissing {
            binary: "claude".to_owned(),
            manual_snippet: "claude mcp add clio".to_owned(),
        })
    };
    let code = run_with(
        Verb::Install,
        &args(&["--harness", "claude"]),
        &|| Ok(fake_exe()),
        &never_direct_write_env,
        &install,
        &never_uninstall,
    );
    assert_eq!(code, ExitCode::from(1));
}

#[test]
fn command_failed_maps_to_exit_1() {
    let install = |_: &HarnessRecord, _: &Path| -> Result<String, DelegateError> {
        Err(DelegateError::CommandFailed {
            exit_code: 7,
            output: "boom".to_owned(),
        })
    };
    let code = run_with(
        Verb::Install,
        &args(&["--harness", "claude"]),
        &|| Ok(fake_exe()),
        &never_direct_write_env,
        &install,
        &never_uninstall,
    );
    assert_eq!(code, ExitCode::from(1));
}

#[test]
fn verification_failed_maps_to_exit_1() {
    let install = |_: &HarnessRecord, _: &Path| -> Result<String, DelegateError> {
        Err(DelegateError::VerificationFailed {
            detail: "no clio server listed".to_owned(),
        })
    };
    let code = run_with(
        Verb::Install,
        &args(&["--harness", "claude"]),
        &|| Ok(fake_exe()),
        &never_direct_write_env,
        &install,
        &never_uninstall,
    );
    assert_eq!(code, ExitCode::from(1));
}

#[test]
fn uninstall_succeeds_without_touching_the_executable_lookup() {
    let calls = RefCell::new(Vec::new());
    let uninstall = |record: &HarnessRecord| -> Result<String, DelegateError> {
        calls.borrow_mut().push(record.name.to_owned());
        Ok("removed clio from claude".to_owned())
    };
    let code = run_with(
        Verb::Uninstall,
        &args(&["--harness", "claude"]),
        &|| panic!("uninstall must not resolve the executable"),
        &never_direct_write_env,
        &never_install,
        &uninstall,
    );
    assert_eq!(code, ExitCode::SUCCESS);
    assert_eq!(*calls.borrow(), vec!["claude".to_owned()]);
}

#[test]
fn binary_missing_message_names_the_binary_and_the_snippet() {
    let message = failure_message(&DelegateError::BinaryMissing {
        binary: "claude".to_owned(),
        manual_snippet: "claude mcp add clio".to_owned(),
    });
    assert!(
        message.contains("`claude`") && message.contains("claude mcp add clio"),
        "{message}"
    );
}

#[test]
fn command_failed_message_carries_the_status_and_output() {
    let message = failure_message(&DelegateError::CommandFailed {
        exit_code: 7,
        output: "boom".to_owned(),
    });
    assert!(
        message.contains("status 7") && message.contains("boom"),
        "{message}"
    );
}

#[test]
fn verification_failed_message_carries_the_detail() {
    let message = failure_message(&DelegateError::VerificationFailed {
        detail: "no clio server listed".to_owned(),
    });
    assert!(message.contains("no clio server listed"), "{message}");
}

#[test]
fn exe_failure_message_says_what_could_not_be_resolved() {
    let error = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
    assert!(exe_failure_message(&error).contains("cannot resolve the clio executable"));
}

#[test]
fn help_text_names_the_purpose_and_every_supported_harness() {
    let text = help_text(Verb::Install);
    assert!(text.contains("Register the clio MCP server"), "{text}");
    assert!(
        text.contains("supported harnesses: claude, codex, opencode, antigravity, hermes, cursor, github copilot"),
        "{text}"
    );
    assert!(
        text.contains("Example: clio install --harness claude"),
        "{text}"
    );
}

#[test]
fn help_text_names_the_antigravity_cli_config_file_for_both_verbs() {
    for verb in [Verb::Install, Verb::Uninstall] {
        let text = help_text(verb);
        assert!(
            text.contains(".gemini/config/mcp_config.json"),
            "{verb:?}: {text}"
        );
        assert!(
            !text.contains("does not write"),
            "{verb:?} must not claim an unverified IDE split: {text}"
        );
    }
}

#[test]
fn antigravity_success_text_names_the_cli_config_file() {
    // `success_text` is the shared report path for both verbs, so one
    // assertion covers the install and the uninstall output.
    let record = agent_harness::resolve("antigravity").expect("antigravity is implemented");
    let text = success_text(record, "registered clio with antigravity");
    assert!(text.contains("registered clio with antigravity"), "{text}");
    assert!(text.contains(".gemini/config/mcp_config.json"), "{text}");
    assert!(
        !text.contains("does not write"),
        "output must not claim an unverified IDE split: {text}"
    );
}

#[test]
fn non_antigravity_success_text_carries_no_config_surface_note() {
    let record = agent_harness::resolve("claude").expect("claude is implemented");
    let text = success_text(record, "registered clio with claude");
    assert_eq!(text, "registered clio with claude");
    assert!(!text.contains("note:"), "{text}");
}
