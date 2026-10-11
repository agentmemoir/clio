// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the registry-fed argument checks over parsed arguments:
//! that a proved flag reads back as its value, and that the read group's
//! registry-fed parser spec still refuses a missing required value, an unknown
//! flag, and a repeated flag while accepting every declared global.
//!
//! ## Owns
//! - `flag`'s read-back behavior over a parsed command line
//! - The registry-fed read spec's refusals and accepted-globals behavior
//!
//! ## Does not own
//! - The required-flags parity over usage lines (`cli_read_args_tests`)
//! - The grammar that produces the parsed arguments (`cli_args_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the argument checks over parsed arguments.

use super::flag;
use crate::cli_read_group::ReadGroup;
use crate::cli_read_test_support::tokens;

/// A flag the required list already proved is present reads back as its value, and
/// an absent one reads as empty rather than panicking, so a forgotten check becomes
/// a message from the tool rather than a crash.
#[test]
fn a_proved_flag_reads_back_as_its_value() {
    let parsed = crate::cli_args::parse(
        &crate::cli_args::FlagSpec::new(&["rule"], &[]),
        ["--rule".to_owned(), "keep".to_owned()],
    )
    .expect("parse");
    assert_eq!(flag(&parsed, "rule"), "keep");
    assert_eq!(flag(&parsed, "absent"), "");
}

// ---------- the registry-fed parser spec ----------

/// A read verb's required value is still refused with its own message once the
/// spec comes from the registry: `compose-context` enforces `--budget-tokens` in
/// its build, not in its record, and that requirement did not move.
#[test]
fn a_registry_fed_spec_still_refuses_a_missing_required_value() {
    let spec = crate::cli_read::spec_for("compose-context").expect("a group spec");
    let parsed = crate::cli_args::parse(&spec, Vec::<String>::new()).expect("the flag is optional");
    let error = crate::cli_read_workspace::WorkspaceGroup
        .build("compose-context", &parsed)
        .expect_err("--budget-tokens is required");
    assert_eq!(error.code(), "usage");
    assert_eq!(
        error.message(),
        "`compose-context` requires a --budget-tokens value"
    );
    assert_eq!(error.hint(), Some("try `--budget-tokens 2000`"));
    assert!(
        super::required_flags("compose-context").is_empty(),
        "the record marks no flag required for this verb"
    );
}

/// An unknown flag on a read verb is still refused, with the same suggestion,
/// once the spec comes from the registry.
#[test]
fn a_registry_fed_spec_still_refuses_an_unknown_flag() {
    let spec = crate::cli_read::spec_for("history temporal").expect("a group spec");
    let error = crate::cli_args::parse(&spec, tokens(&["t", "--time-axiss", "valid"]))
        .expect_err("an unknown flag fails closed");
    assert_eq!(error.code(), "usage");
    assert_eq!(error.message(), "unknown flag `--time-axiss`");
    assert_eq!(error.hint(), Some("did you mean `--time-axis`?"));
}

/// A value flag given twice is still refused rather than last-wins, once the
/// spec comes from the registry.
#[test]
fn a_registry_fed_spec_still_refuses_a_repeated_flag() {
    let spec = crate::cli_read::spec_for("task history").expect("a group spec");
    let error = crate::cli_args::parse(&spec, tokens(&["t1", "--limit", "1", "--limit", "2"]))
        .expect_err("a repeated flag is refused");
    assert_eq!(error.code(), "usage");
    assert_eq!(error.message(), "--limit was given more than once");
    assert_eq!(error.hint(), Some("pass --limit once"));
}

/// Every declared global is still accepted by a read verb's spec once it comes
/// from the registry, in the value and boolean forms alike.
#[test]
fn a_registry_fed_spec_still_accepts_the_globals() {
    let spec = crate::cli_read::spec_for("memtree query").expect("a group spec");
    let mut args: Vec<String> = Vec::new();
    for name in crate::cli_command_registry::global_value_names() {
        args.push(format!("--{name}"));
        args.push("value".to_owned());
    }
    for name in crate::cli_command_registry::global_bool_names() {
        args.push(format!("--{name}"));
    }
    let parsed = crate::cli_args::parse(&spec, args).expect("every declared global parses");
    for name in crate::cli_command_registry::global_value_names() {
        assert_eq!(parsed.value(name), Some("value"), "{name}");
    }
    for name in crate::cli_command_registry::global_bool_names() {
        assert!(parsed.has(name), "{name}");
    }
}
