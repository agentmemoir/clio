// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the shared CLI error contract: constructors, exit codes,
//! credential masking, and the envelope/human renderings.
//!
//! ## Owns
//! - Assertions over `CliError`
//!
//! ## Does not own
//! - The error implementation under test (`cli_error`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use clio_types::{AmError, ErrorCode};
use std::process::ExitCode;

#[test]
fn usage_error_exits_two_with_usage_code() {
    let error = CliError::usage("bad flag");
    assert_eq!(error.code(), "usage");
    assert_eq!(error.exit_code(), ExitCode::from(2));
    assert!(error.hint().is_none());
}

#[test]
fn usage_hint_carries_the_hint() {
    let error = CliError::usage_hint("missing query", "try `clio recall QUERY`");
    assert_eq!(error.hint(), Some("try `clio recall QUERY`"));
    assert_eq!(error.exit_code(), ExitCode::from(2));
}

#[test]
fn help_hint_only_fills_an_unhinted_usage_error() {
    let usage = with_help_hint(CliError::usage("bad flag"), "recall");
    assert_eq!(usage.hint(), Some("try `clio recall --help`"));

    let suggested = CliError::usage_hint("bad flag", "try --bank");
    let usage = with_help_hint(suggested, "recall");
    assert_eq!(usage.hint(), Some("try --bank"));

    let operational = with_help_hint(CliError::operational("internal", "failed"), "recall");
    assert!(operational.hint().is_none());
}

#[test]
fn operational_error_exits_one_with_the_given_code() {
    let error = CliError::operational("internal", "boom");
    assert_eq!(error.code(), "internal");
    assert_eq!(error.message(), "boom");
    assert_eq!(error.exit_code(), ExitCode::from(1));
}

#[test]
fn storage_lock_conflicts_have_a_temporary_exit_code() {
    let error = AmError::new(
        ErrorCode::StorageBusy,
        "Postgres lock conflict; retry the command",
    );
    for mapped in [
        CliError::from_am_error(&error),
        CliError::tool_failure("storage_busy", &error.message),
    ] {
        assert_eq!(mapped.exit_code(), ExitCode::from(75));
        assert_eq!(mapped.to_envelope()["code"], "storage_busy");
    }
}

#[test]
fn tool_domain_errors_keep_the_operational_exit_group() {
    for code in ["invalid_argument", "invalid_type", "out_of_range"] {
        let error = CliError::tool_failure(code, "bad flag value");
        assert_eq!(error.code(), code);
        assert_eq!(error.exit_code(), ExitCode::from(1));
    }
    let missing = CliError::tool_failure("item_not_found", "item not found");
    assert_eq!(missing.code(), "item_not_found");
    assert_eq!(missing.exit_code(), ExitCode::from(1));
}

#[test]
fn from_am_error_keeps_code_and_masks_credentials() {
    let am = AmError::new(
        ErrorCode::Internal,
        "connect postgres://user:s3cret@127.0.0.1:5432/db failed",
    );
    let error = CliError::from_am_error(&am);
    assert_eq!(error.code(), "internal");
    assert_eq!(error.exit_code(), ExitCode::from(1));
    assert!(!error.message().contains("s3cret"), "{}", error.message());
    assert!(error.message().contains("postgres://***@127.0.0.1:5432/db"));
}

#[test]
fn an_operational_failure_masks_credentials_in_a_foreign_message() {
    let am = AmError::new(
        ErrorCode::Internal,
        "the store at postgres://user:s3cret@127.0.0.1:5432/db cannot be measured",
    );
    let error = CliError::from_am_error(&am);
    assert_eq!(error.exit_code(), ExitCode::from(1));
    assert_eq!(error.code(), "internal");
    assert!(!error.message().contains("s3cret"), "{}", error.message());
    assert!(
        error.message().contains("postgres://***@127.0.0.1:5432/db"),
        "{}",
        error.message()
    );
}

/// The restore's exit codes come from one table, so a failure is built with a
/// generic constructor and re-mapped to the table's code. The re-mapped message
/// must go through the redactor again, or a table-built failure could leak a
/// credential that the generic constructor would have masked.
#[test]
fn a_remapped_exit_code_masks_credentials_again() {
    let error = CliError::operational(
        "internal",
        "cannot reach postgres://user:s3cret@127.0.0.1:5432/db",
    )
    .with_exit(2);

    assert_eq!(error.exit_code(), ExitCode::from(2));
    assert!(!error.message().contains("s3cret"), "{}", error.message());
}

#[test]
fn envelope_omits_absent_hint_and_includes_present_hint() {
    let plain = CliError::usage("bad").to_envelope();
    assert_eq!(plain["ok"], serde_json::json!(false));
    assert_eq!(plain["code"], serde_json::json!("usage"));
    assert!(plain.get("hint").is_none());

    let hinted = CliError::usage("bad").with_hint("do better").to_envelope();
    assert_eq!(hinted["hint"], serde_json::json!("do better"));
}

#[test]
fn human_rendering_adds_a_hint_line() {
    assert_eq!(CliError::usage("bad").to_human(), "error: bad");
    assert_eq!(
        CliError::usage("bad").with_hint("do better").to_human(),
        "error: bad\nhint: do better"
    );
}

#[test]
fn usage_help_carries_the_block_and_exits_two() {
    let error = CliError::usage_help("missing required option '--x'", "Usage: clio foo");
    assert_eq!(error.code(), "usage");
    assert_eq!(error.exit_code(), ExitCode::from(2));
    assert!(error.hint().is_none());
    assert_eq!(error.help(), Some("Usage: clio foo"));
}

#[test]
fn human_rendering_puts_the_raw_help_block_before_the_hint() {
    // The block is printed verbatim: no `hint:` label, one blank line after the
    // message.
    assert_eq!(
        CliError::usage_help("boom", "Usage: clio foo\n  clio foo --x")
            .with_hint("try --x")
            .to_human(),
        "error: boom\n\nUsage: clio foo\n  clio foo --x\nhint: try --x"
    );
}

#[test]
fn envelope_keeps_message_short_and_carries_help_separately() {
    let envelope =
        CliError::usage_help("missing required option '--x'", "Usage: clio foo").to_envelope();
    assert_eq!(
        envelope["message"],
        serde_json::json!("missing required option '--x'")
    );
    assert!(envelope.get("hint").is_none());
    assert_eq!(envelope["help"], serde_json::json!("Usage: clio foo"));
}

#[test]
fn cross_process_dek_hint_names_the_durable_provider() {
    let hint = cross_process_dek_hint("no DEK for subject `itm-1`").expect("hint");
    assert!(hint.contains("durable KMS provider"), "{hint}");
    assert!(cross_process_dek_hint("item not found").is_none());
}

#[test]
fn library_input_names_are_translated_for_cli_messages() {
    assert_eq!(
        name_cli_inputs("retrieve item_id is required"),
        "the ID argument is required"
    );
    assert_eq!(
        name_cli_inputs("the store is unavailable"),
        "the store is unavailable"
    );
}

#[test]
fn blank_flag_values_are_rejected_with_a_replacement_hint() {
    assert!(reject_blank_value("--bank", "agents").is_ok());

    for value in ["", "  \t  "] {
        let error = reject_blank_value("--bank", value).expect_err("blank values are refused");
        assert_eq!(error.code(), "usage");
        assert_eq!(error.hint(), Some("pass a non-blank value or omit --bank"));
    }
}

#[test]
fn blank_positional_values_are_rejected() {
    assert!(reject_blank_positional("remember this").is_ok());

    let error = reject_blank_positional(" \t ").expect_err("blank text is refused");
    assert_eq!(error.code(), "usage");
    assert_eq!(
        error.hint(),
        Some("pass the text, or leave the argument out")
    );
}

#[test]
fn repeated_flags_are_rejected_only_after_the_first_value() {
    assert!(reject_repeated_flag("--output", false).is_ok());

    let error = reject_repeated_flag("--output", true).expect_err("second value is refused");
    assert_eq!(error.message(), "--output was given more than once");
    assert_eq!(error.hint(), Some("pass --output once"));
}

#[test]
fn accepted_timestamp_forms_pass_and_malformed_values_are_refused() {
    for value in ["2026-01-01T00:00:00Z", "2026-01-01", "2026-01-01T00:00:00"] {
        assert!(reject_bad_timestamp("--since", value).is_ok(), "{value}");
    }

    let error = reject_bad_timestamp("--since", "tomorrow").expect_err("not a timestamp");
    assert_eq!(error.code(), "usage");
    assert_eq!(
        error.hint(),
        Some("pass a timestamp for --since, or omit it")
    );
}
