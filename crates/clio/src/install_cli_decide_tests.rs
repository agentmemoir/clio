// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the parse-and-decide step of `clio install` and
//! `clio uninstall`: help, the portable-only decision, harness resolution, the
//! usage errors, and the `--force` flag each verb does or does not accept.
//!
//! ## Owns
//! - Assertions over `decide` for both verbs, including every usage error it
//!   returns and the `force` field it sets
//!
//! ## Does not own
//! - The decision implementation under test (`install_cli_decide`)
//! - Running a decision, the exit codes, and the help text
//!   (`install_cli_tests`)
//!
//! ## Boundary
//!
//! Test-only. Every assertion reads a pure parse step, so no test captures
//! stdout, spawns a process, or touches a file.

use super::*;
use crate::agent_harness::Mechanism;
use std::process::ExitCode;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|item| (*item).to_owned()).collect()
}

#[test]
fn help_with_an_explicit_value_is_answered_through_parse_flags() {
    // `--help=x` is not one of the exact tokens `wants_help` matches, so it
    // must reach the help decision through the parsed `help` flag instead.
    let decision =
        decide(Verb::Install, &args(&["--help=x"])).expect("--help=x is a valid help spelling");
    assert!(matches!(decision, Decision::Help), "{decision:?}");
}

#[test]
fn unknown_harness_is_refused_with_the_supported_list() {
    let error =
        decide(Verb::Install, &args(&["--harness", "vim"])).expect_err("vim is not supported");
    assert_eq!(error.exit_code(), ExitCode::from(2));
    assert!(
        error.message().contains("unknown harness `vim`"),
        "{error:?}"
    );
    assert!(error.hint().unwrap_or_default().contains("claude"));
}

#[test]
fn gh_copilot_is_an_unknown_harness_usage_error_with_the_supported_list() {
    let error = decide(Verb::Install, &args(&["--harness", "gh-copilot"]))
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
    let decision =
        decide(Verb::Install, &args(&["--harness", "cursor"])).expect("cursor is supported");
    assert!(
        matches!(decision, Decision::Run { record, .. } if record.mechanism == Mechanism::DirectWrite),
        "{decision:?}"
    );
}

#[test]
fn vs_code_alias_resolves_to_the_github_copilot_direct_write_record() {
    let decision = decide(Verb::Install, &args(&["--harness", "vs code"]))
        .expect("the vs code alias resolves");
    assert!(
        matches!(decision, Decision::Run { record, .. } if record.name == "github copilot"
            && record.mechanism == Mechanism::DirectWrite),
        "{decision:?}"
    );
}

#[test]
fn github_copilot_resolves_to_an_installable_direct_write_record() {
    let decision = decide(Verb::Install, &args(&["--harness", "github copilot"]))
        .expect("github copilot is supported");
    assert!(
        matches!(decision, Decision::Run { record, .. } if record.mechanism == Mechanism::DirectWrite),
        "{decision:?}"
    );
}

#[test]
fn canonical_name_resolves_to_its_record() {
    let decision =
        decide(Verb::Install, &args(&["--harness", "claude"])).expect("claude is supported");
    assert!(
        matches!(decision, Decision::Run { record, .. } if record.name == "claude"),
        "{decision:?}"
    );
}

#[test]
fn alias_and_capitalization_resolve_to_the_canonical_record() {
    let decision =
        decide(Verb::Install, &args(&["--harness", "Claude-Code"])).expect("alias must resolve");
    assert!(
        matches!(decision, Decision::Run { record, .. } if record.name == "claude"),
        "{decision:?}"
    );
}

#[test]
fn a_plain_install_and_a_plain_portable_install_carry_no_force() {
    let harness = decide(Verb::Install, &args(&["--harness", "claude"])).expect("claude resolves");
    assert!(
        matches!(harness, Decision::Run { force: false, .. }),
        "{harness:?}"
    );
    let portable = decide(Verb::Install, &args(&[])).expect("no --harness is the portable install");
    assert!(
        matches!(portable, Decision::Portable { force: false }),
        "{portable:?}"
    );
}

#[test]
fn install_parses_force_into_the_decision_for_both_shapes() {
    let harness = decide(Verb::Install, &args(&["--harness", "claude", "--force"]))
        .expect("--force is accepted");
    assert!(
        matches!(harness, Decision::Run { force: true, .. }),
        "{harness:?}"
    );
    let portable =
        decide(Verb::Install, &args(&["--force"])).expect("--force with no harness is portable");
    assert!(
        matches!(portable, Decision::Portable { force: true }),
        "{portable:?}"
    );
}

#[test]
fn uninstall_refuses_force_because_its_record_does_not_document_it() {
    let error = decide(Verb::Uninstall, &args(&["--harness", "claude", "--force"]))
        .expect_err("uninstall documents no --force");
    assert_eq!(
        error.exit_code(),
        ExitCode::from(2),
        "an unknown flag is a usage error"
    );
    assert!(error.message().contains("force"), "{error:?}");
}
