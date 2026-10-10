// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the shared `--category` usage help for the core write
//! commands: the missing-`--category` error printed by `remember` and `admit`.
//!
//! ## Owns
//! - Golden assertions that the error is exit 2, keeps `message` short, and
//!   carries the valid categories and descriptions in a separate `help` block
//!
//! ## Does not own
//! - The commands under test (`cli_write_core`)
//! - The rest of the core write-command suite (see `cli_write_core_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_test_support::state;
use crate::cli_write_test_support::run_write_state;
use clio_types::SemanticCategory;
use serde_json::{Value, json};
use std::process::ExitCode;

/// Parse the JSON-mode error envelope printed to stderr.
fn error_envelope(err: &str) -> Value {
    serde_json::from_str(err).expect("error envelope is JSON")
}

#[test]
fn admit_without_category_keeps_message_short_and_lists_categories_in_help() {
    let s = state();
    let (code, out, err) = run_write_state("admit", &["some gist"], &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    let envelope = error_envelope(&err);
    assert_eq!(envelope["code"], json!("usage"));
    assert_eq!(
        envelope["message"],
        json!("missing required option '--category <value>'")
    );
    assert!(envelope.get("hint").is_none(), "{err}");
    let help = envelope["help"].as_str().expect("help block");
    assert!(
        help.contains("Usage: clio admit TEXT --category <category>"),
        "{help}"
    );
    for &(_category, wire, description) in SemanticCategory::ALL {
        assert!(help.contains(wire), "missing {wire}: {help}");
        assert!(help.contains(description), "missing {description}: {help}");
    }
    // This help block is read by people outside this repository, so it must not
    // cite a label from our own design notes. The check is on the shape of a
    // label rather than on one known label, so a new one cannot slip back in
    // either.
    for word in help.split_whitespace() {
        let word = word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
        let Some((tag, number)) = word.split_once('-') else {
            continue;
        };
        let is_label = !tag.is_empty()
            && tag.len() <= 3
            && tag.chars().all(|c| c.is_ascii_uppercase())
            && !number.is_empty()
            && number.chars().all(|c| c.is_ascii_digit());
        assert!(!is_label, "design label `{word}` in the help block: {help}");
    }
}

#[test]
fn remember_without_category_prints_the_golden_help_block() {
    let s = state();
    let (code, out, err) = run_write_state("remember", &["some gist"], &s, true);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert_eq!(
        err,
        concat!(
            "error: missing required option '--category <value>'\n\n",
            "Usage: clio remember TEXT --category <category>\n\n",
            "Valid Categories:\n",
            "  \u{2022} task_spec         Task specifications\n",
            "  \u{2022} schema            Data schemas\n",
            "  \u{2022} tool_config       Tool configurations\n",
            "  \u{2022} output_constraint Output constraints\n",
            "  \u{2022} persona           User model\n\n",
            "Example:\n",
            "  clio remember \"Bob lives in Canada\" --category persona\n",
        ),
        "{err}"
    );
}

#[test]
fn help_block_uses_the_shared_category_list() {
    // The block is built from `SemanticCategory::ALL`, so every whitelist
    // category and its description must appear.
    let block = super::missing_category_help("admit");
    for &(category, _wire, description) in SemanticCategory::ALL {
        let row = format!("  \u{2022} {:<18}{description}", category.as_str());
        assert!(block.contains(&row), "missing row `{row}` in:\n{block}");
    }
}

/// Regression cover for issue #39: an invalid `--category` value was passed
/// through unvalidated, so the domain layer rejected it as `invalid_argument`
/// and the command exited 1. A bad value for a caller-supplied flag is the
/// same class of mistake as a bad `--epistemic-kind` or an unknown flag, and
/// both of those are a usage error with exit 2. This flag was the outlier.
#[test]
fn an_invalid_category_is_a_usage_error_not_an_operational_one() {
    let s = state();
    let (code, out, err) =
        run_write_state("remember", &["some gist", "--category", "nope"], &s, false);
    assert_eq!(
        code,
        ExitCode::from(2),
        "a bad flag value must exit 2 like every other bad flag value: {out}{err}"
    );
    let envelope = error_envelope(&err);
    assert_eq!(
        envelope["code"],
        json!("usage"),
        "not an operational failure: {err}"
    );
    assert!(
        envelope["message"]
            .as_str()
            .expect("message")
            .contains("--category must be"),
        "{err}"
    );
}

/// The refusal lists exactly the whitelist, and the hint points at a real
/// member of it, so the message cannot drift from the enum it validates
/// against. `#39` is the reason the list is read from `SemanticCategory::WIRE`
/// rather than written out again here.
#[test]
fn an_invalid_category_lists_the_whole_whitelist() {
    let s = state();
    let (_, _, err) = run_write_state("remember", &["some gist", "--category", "nope"], &s, false);
    let envelope = error_envelope(&err);
    let message = envelope["message"].as_str().expect("message");
    for &wire in SemanticCategory::WIRE {
        assert!(message.contains(wire), "missing {wire} from: {message}");
    }
    let hint = envelope["hint"].as_str().expect("a hint");
    // `bogus` is close to nothing, so the hint lists every real value instead of
    // pointing at the first one however wrong it is for the caller.
    assert!(
        hint.starts_with("--category takes one of: "),
        "an unmatched value must list the real ones, got `{hint}`"
    );
    for &category in SemanticCategory::WIRE {
        assert!(hint.contains(category), "`{hint}` must offer {category}");
    }
}
