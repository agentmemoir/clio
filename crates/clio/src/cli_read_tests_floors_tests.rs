// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the CLI `recall` binding of the optional score floors and
//! the two entity-inclusion controls: which `retrieve` argument each flag sets,
//! that a flag which was not given contributes no key at all, that the default
//! call is unchanged, that a bad floor value fails closed as a usage error, and
//! that asking for an entity summary with entity names suppressed is refused by
//! the tool rather than silently honored.
//!
//! ## Owns
//! - Flag-to-argument assertions for `min_scores`, `include_entities`, and
//!   `entity_summary` on `recall`
//! - The bad-value, contradictory-controls, and unknown-flag failures
//!
//! ## Does not own
//! - The floor and entity-control rules themselves (`clio-retrieve` floors,
//!   entity summary, and the retrieval page mapper)
//! - The end-to-end spawned-binary behavior (`recall_floors_harness`)
//! - The read-command runners and seed fixtures (shared from `cli_read_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the CLI flag mapping.

use super::*;
use crate::cli_read_core::CoreGroup;
use crate::cli_read_group::ReadGroup;
use serde_json::json;

/// The `recall` floor flags, with the `min_scores` key each one sets.
const FLOOR_FLAGS: [(&str, &str); 4] = [
    ("--min-semantic", "semantic"),
    ("--min-keyword", "keyword"),
    ("--min-reranker", "reranker"),
    ("--min-final", "final"),
];

/// The `retrieve` tool name and argument object for one `recall` invocation.
///
/// This is the binding itself, with no dispatcher in the way, so a failure names
/// the flag-to-argument mapping rather than the engine around it.
fn recall_args(args: &[&str]) -> Result<Value, CliError> {
    let group = CoreGroup;
    let spec = group.spec("recall");
    let parsed = crate::cli_args::parse(&spec, tokens(args))?;
    let (tool, arguments) = group.build("recall", &parsed)?;
    assert_eq!(tool, "retrieve", "the verb keeps its tool binding");
    Ok(arguments)
}

/// The keys of the `min_scores` object, sorted, for one invocation.
fn min_scores_keys(args: &Value) -> Vec<String> {
    let mut keys: Vec<String> = args["min_scores"]
        .as_object()
        .expect("min_scores object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// Each floor flag on its own sets exactly its own `min_scores` key. A flag
/// that was not given must leave the object absent rather than set to a
/// sentinel, because an unsupplied floor is what imposes no floor at all.
#[test]
fn recall_each_floor_flag_sets_only_its_own_key() {
    for (flag, key) in FLOOR_FLAGS {
        let args = recall_args(&["q", flag, "0.5"]).expect("build");
        assert_eq!(args["min_scores"][key], json!(0.5), "{flag}");
        assert_eq!(min_scores_keys(&args), [key], "{flag} adds no other key");
    }
}

/// All four floors together build exactly the documented `min_scores` object,
/// under the same four wire names every other binding uses.
#[test]
fn recall_all_four_floors_use_the_documented_key_names() {
    let args = recall_args(&[
        "q",
        "--min-semantic",
        "0.2",
        "--min-keyword",
        "3",
        "--min-reranker",
        "0.4",
        "--min-final",
        "1.5",
    ])
    .expect("build");
    assert_eq!(
        args["min_scores"],
        json!({"semantic": 0.2, "keyword": 3.0, "reranker": 0.4, "final": 1.5})
    );
    assert_eq!(
        min_scores_keys(&args),
        ["final", "keyword", "reranker", "semantic"]
    );
}

/// One supplied floor does not fill in the other three, in either the
/// space-separated or the `--flag=value` form.
#[test]
fn recall_one_floor_leaves_the_others_unset() {
    for spelled in [
        vec!["q", "--min-reranker", "0.75"],
        vec!["q", "--min-reranker=0.75"],
    ] {
        let args = recall_args(&spelled).expect("build");
        assert_eq!(args["min_scores"], json!({"reranker": 0.75}), "{spelled:?}");
    }
}

/// The default call is unchanged: no `min_scores` object, no `include_entities`,
/// and no `entity_summary`, so the tool applies its own documented defaults.
#[test]
fn recall_default_call_sends_no_optional_argument() {
    let args = recall_args(&["q", "--bank", "bank-a", "--limit", "5"]).expect("build");
    let object = args.as_object().expect("arguments object");
    for key in ["min_scores", "include_entities", "entity_summary"] {
        assert!(!object.contains_key(key), "{key} must be absent: {args}");
    }
    assert_eq!(args["query"], json!("q"));
    assert_eq!(args["bank"], json!("bank-a"));
    assert_eq!(args["limit"], json!(5));
}

/// `--no-entities` is the negative form of `include_entities`, so it sets that
/// argument to `false` and touches nothing else. The default stays `true` in
/// the tool, so the flag is never needed to ask for entity names.
#[test]
fn recall_no_entities_sets_include_entities_false() {
    let args = recall_args(&["q", "--no-entities"]).expect("build");
    assert_eq!(args["include_entities"], json!(false), "{args}");
    assert!(args.get("entity_summary").is_none(), "{args}");
    assert!(args.get("min_scores").is_none(), "{args}");
}

/// `--entity-summary` opts into the top-level summary, which the tool leaves off
/// by default.
#[test]
fn recall_entity_summary_sets_entity_summary_true() {
    let args = recall_args(&["q", "--entity-summary"]).expect("build");
    assert_eq!(args["entity_summary"], json!(true), "{args}");
    assert!(args.get("include_entities").is_none(), "{args}");
    assert!(args.get("min_scores").is_none(), "{args}");
}

/// A floor value the CLI cannot use is a usage error before the tool is called:
/// either it is not a finite number, or it is negative and no inclusive floor
/// could ever be met by it. The message names the flag, and the raw value is
/// echoed the way `--limit` echoes an unparseable value.
#[test]
fn recall_rejects_unparseable_and_negative_floors() {
    let state = state_with_items();
    let cases: &[(&str, &str)] = &[
        ("--min-final", "high"),
        ("--min-semantic", "0.5x"),
        ("--min-keyword", "NaN"),
        ("--min-reranker", "inf"),
        ("--min-semantic", "-0.5"),
        ("--min-keyword", "-1"),
        ("--min-reranker", "-0.0001"),
        ("--min-final", "-100"),
    ];
    for &(flag, value) in cases {
        let (code, out, err) = run_state(
            "recall",
            &["q", flag, value, "--bank", "bank-a"],
            &state,
            false,
        );
        assert_eq!(code, ExitCode::from(2), "{flag} {value}: {err}");
        assert!(out.is_empty(), "{flag} {value} stdout: {out}");
        assert!(
            err.contains("\"code\":\"usage\""),
            "{flag} {value} stderr: {err}"
        );
        assert!(err.contains(flag), "{flag} must be named: {err}");
    }
}

/// The CLI accepts both controls, but a summary is made of entity names, so the
/// tool refuses the combination. The call fails closed: no payload, a non-zero
/// exit, and an error that names the contradiction rather than dropping either
/// control.
#[test]
fn recall_no_entities_with_entity_summary_fails_closed() {
    let state = state_with_items();
    let (code, out, err) = run_state(
        "recall",
        &[
            "terse",
            "--no-entities",
            "--entity-summary",
            "--bank",
            "bank-a",
        ],
        &state,
        false,
    );
    assert_ne!(code, ExitCode::from(0), "the call must not succeed: {out}");
    assert!(out.is_empty(), "no payload on failure: {out}");
    let envelope = json_of(&err);
    assert_eq!(envelope["ok"], json!(false), "{err}");
    let message = envelope["message"].as_str().expect("error message");
    assert!(message.contains("--entity-summary"), "{err}");
    assert!(message.contains("--no-entities"), "{err}");
}

/// A misspelled floor flag is still an unknown flag, so the parser fails closed
/// with a usage error and a suggestion rather than ignoring it.
#[test]
fn recall_rejects_an_unknown_floor_flag() {
    let state = state_with_items();
    for flag in [
        "--min-score",
        "--min-semantics",
        "--entities",
        "--min-semantic-x",
    ] {
        let (code, out, err) = run_state(
            "recall",
            &["q", flag, "0.5", "--bank", "bank-a"],
            &state,
            false,
        );
        assert_eq!(code, ExitCode::from(2), "{flag}: {err}");
        assert!(out.is_empty(), "{flag} stdout: {out}");
        assert!(
            err.contains("\"code\":\"usage\"") && err.contains("unknown flag"),
            "{flag} stderr: {err}"
        );
    }
}

/// The two entity controls are boolean flags, so a value attached to one is a
/// usage error rather than a silently ignored `=false`. The canonical spelling
/// is the bare flag, and `--no-entities` is already the negative form.
#[test]
fn recall_entity_flag_rejects_an_attached_value() {
    let state = state_with_items();
    for flag in ["--entity-summary=false", "--no-entities=true"] {
        let (code, out, err) = run_state(
            "recall",
            &["terse", flag, "--bank", "bank-a"],
            &state,
            false,
        );
        assert_eq!(code, ExitCode::from(2), "{flag}: {err}");
        assert!(out.is_empty(), "{flag} stdout: {out}");
        assert!(
            err.contains("\"code\":\"usage\"") && err.contains("takes no value"),
            "{flag} stderr: {err}"
        );
    }
}

/// The usage line lists the new flags, so `clio recall --help` documents them
/// and repeats that a floor is a relative filter rather than a calibrated
/// relevance threshold.
#[test]
fn recall_help_lists_the_floor_and_entity_flags() {
    let (code, out, err) = run_open("recall", &["--help"]);
    assert_eq!(code, ExitCode::from(0), "stderr: {err}");
    for flag in [
        "--min-semantic",
        "--min-keyword",
        "--min-reranker",
        "--min-final",
        "--no-entities",
        "--entity-summary",
    ] {
        assert!(out.contains(flag), "{flag} must be documented: {out}");
    }
    assert!(out.contains("not a calibrated threshold"), "{out}");
}
