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

/// `install`'s one-line purpose, which the help screen opens with.
#[test]
fn install_purpose_says_it_registers_the_server() {
    assert_eq!(
        Verb::Install.purpose(),
        "Register the clio MCP server with a supported agent harness."
    );
}

/// `uninstall`'s one-line purpose. It is the removal, not the registration, so
/// the two verbs cannot be confused for one another in help.
#[test]
fn uninstall_purpose_says_it_removes_the_registration() {
    assert_eq!(
        Verb::Uninstall.purpose(),
        "Remove the clio MCP server registration from a supported agent harness."
    );
}

/// `uninstall --skill-only` with no `--harness` removes the portable copy, so
/// the decision names no harness record at all.
#[test]
fn skill_only_with_no_harness_is_the_portable_removal() {
    let decision =
        decide(Verb::Uninstall, &args(&["--skill-only"])).expect("--skill-only is documented");
    assert!(
        matches!(decision, Decision::SkillOnly { record: None }),
        "{decision:?}"
    );
}

/// `uninstall --skill-only --harness claude` still removes only the package; it
/// names the record so the removal targets that harness's skill root.
#[test]
fn skill_only_with_a_harness_names_that_record() {
    let decision = decide(
        Verb::Uninstall,
        &args(&["--skill-only", "--harness", "claude"]),
    )
    .expect("--skill-only with a supported harness is valid");
    assert!(
        matches!(decision, Decision::SkillOnly { record: Some(record) } if record.name == "claude"),
        "{decision:?}"
    );
}

/// Uninstall has no portable-only shape, so a call with neither `--harness` nor
/// `--skill-only` is refused rather than guessed at.
#[test]
fn uninstall_with_no_harness_is_the_missing_harness_usage_error() {
    let error = decide(Verb::Uninstall, &args(&[])).expect_err("uninstall requires a harness");
    assert_eq!(error.code(), "usage");
    assert_eq!(error.exit_code(), ExitCode::from(2));
    assert_eq!(error.message(), "--harness is required");
    assert!(
        error
            .hint()
            .unwrap_or_default()
            .starts_with("supported harnesses: claude,"),
        "{error:?}"
    );
}

/// Every name the registry knows decides to a run. `resolve_harness` refuses a
/// name as known-but-unsupported only when `agent_harness::normalize` names it
/// while `agent_harness::resolve` holds no record for that name, and the
/// registry serves a record for every name it normalizes, so no accepted name —
/// canonical or alias — reaches that refusal from `decide`. The property is
/// asserted rather than the refusal because the refusal has no reachable input.
#[test]
fn every_name_the_registry_knows_is_an_install_rather_than_a_known_refusal() {
    let names = agent_harness::supported_names();
    assert!(!names.is_empty(), "the registry offers a name to check");
    for name in names {
        let record = agent_harness::resolve(name).expect("a supported name has a record");
        for accepted in std::iter::once(name).chain(record.aliases.iter().copied()) {
            let decision = decide(Verb::Install, &args(&["--harness", accepted]))
                .unwrap_or_else(|error| panic!("`{accepted}` is a known name: {error:?}"));
            assert!(
                matches!(decision, Decision::Run { record: resolved, .. } if resolved.name == record.name),
                "`{accepted}` must run `{}`: {decision:?}",
                record.name
            );
        }
    }
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

/// `--harness` needs a value, so a help spelling after it is not a help request:
/// both verbs report the missing value the parser refuses the argv with.
///
/// Reading help before parsing made `install --harness -h` print a help screen
/// while `version --check -h` reported the missing value, so one argv shape had
/// two answers depending on the surface.
#[test]
fn a_help_spelling_after_harness_is_the_missing_value() {
    for (verb, spelling) in [
        (Verb::Install, "-h"),
        (Verb::Install, "--help"),
        (Verb::Uninstall, "-h"),
        (Verb::Uninstall, "--help"),
    ] {
        let error = decide(verb, &args(&["--harness", spelling]))
            .expect_err("`--harness` claims the token after it, so there is no value");
        assert_eq!(error.exit_code(), ExitCode::from(2), "{}", verb.label());
        assert_eq!(
            error.message(),
            "flag `--harness` requires a value",
            "{} {spelling}",
            verb.label()
        );
    }
    // A valueless flag claims nothing, so the spelling behind it still asks.
    for argv in [vec!["--force", "-h"], vec!["--skill-only", "--help"]] {
        assert!(
            matches!(decide(Verb::Install, &args(&argv)), Ok(Decision::Help)),
            "{argv:?} asks for help"
        );
    }
    // With a value given, the spelling behind it asks for help again.
    assert!(matches!(
        decide(Verb::Install, &args(&["--harness", "claude", "-h"])),
        Ok(Decision::Help)
    ));
}
