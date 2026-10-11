// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the CLI `--query-anchor` binding: which `retrieve` /
//! `compose_context` argument the flag sets, that a call without it sends no
//! key at all, that the raw string (offset included) passes through unchanged,
//! and that a misspelled flag still fails closed as a usage error.
//!
//! ## Owns
//! - Flag-to-argument assertions for `query_anchor` on `recall` and
//!   `compose-context`
//! - The absent-key and unknown-flag behaviors
//! - That `CoreGroup::spec` answers each core verb with its registry record's
//!   parser spec
//!
//! ## Does not own
//! - The window rules and anchor validation (`clio-retrieve` `temporal`)
//! - The read-command runners and seed fixtures (`cli_read_tests`)
//! - The score-floor and entity-inclusion flags (`cli_read_floors_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the CLI temporal-anchor flag mapping.

use crate::cli_error::CliError;
use crate::cli_read_core::CoreGroup;
use crate::cli_read_group::ReadGroup;
use crate::cli_read_workspace::WorkspaceGroup;
use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_mcp::write_tools::dispatch;
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};
use std::process::ExitCode;
use std::sync::Arc;

/// An anchor with an explicit non-UTC offset, so passthrough is observable.
const OFFSET_ANCHOR: &str = "2026-09-26T05:00:00-07:00";
/// A second anchor, used where the value itself is not the point.
const ANCHOR: &str = "2026-09-26T12:00:00Z";
/// The hermetic bank the payload-equality tests run against.
const BANK: &str = "temporal-cli";
/// The query the payload-equality fixtures are built around.
const WINDOW_QUERY: &str = "what happened this week";

fn tokens(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

/// The `retrieve` tool name and argument object for one `recall` invocation.
///
/// This is the binding itself, with no dispatcher in the way, so a failure
/// names the flag-to-argument mapping rather than the engine around it.
fn recall_args(args: &[&str]) -> Result<Value, CliError> {
    let group = CoreGroup;
    let parsed = crate::cli_args::parse(&group.spec("recall"), tokens(args))?;
    let (tool, arguments) = group.build("recall", &parsed)?;
    assert_eq!(tool, "retrieve", "the verb keeps its tool binding");
    Ok(arguments)
}

/// The `compose_context` tool name and argument object for one invocation.
fn compose_args(args: &[&str]) -> Result<Value, CliError> {
    let group = WorkspaceGroup;
    let parsed = crate::cli_args::parse(&group.spec("compose-context"), tokens(args))?;
    let (tool, arguments) = group.build("compose-context", &parsed)?;
    assert_eq!(tool, "compose_context", "the verb keeps its tool binding");
    Ok(arguments)
}

/// The argument keys of one built call, sorted, for "no other key" assertions.
fn argument_keys(args: &Value) -> Vec<String> {
    let mut keys: Vec<String> = args
        .as_object()
        .expect("arguments object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// `--query-anchor` on `recall` sets exactly `query_anchor`, in both the
/// space-separated and the `--flag=value` form, and adds no other key.
#[test]
fn recall_query_anchor_sets_only_query_anchor() {
    for spelled in [
        vec!["q", "--query-anchor", ANCHOR],
        vec!["q", "--query-anchor=2026-09-26T12:00:00Z"],
    ] {
        let args = recall_args(&spelled).expect("build");
        assert_eq!(args["query_anchor"], json!(ANCHOR), "{spelled:?}");
        assert_eq!(
            argument_keys(&args),
            ["query", "query_anchor"],
            "{spelled:?} adds no other key"
        );
    }
}

/// Without the flag the key is absent entirely, not `null`, so the tool applies
/// its documented no-anchor behavior.
#[test]
fn recall_without_the_anchor_leaves_the_key_absent() {
    let args = recall_args(&["q", "--bank", "bank-a"]).expect("build");
    assert!(
        !args
            .as_object()
            .expect("arguments object")
            .contains_key("query_anchor"),
        "absent, not null: {args}"
    );
    assert_eq!(argument_keys(&args), ["bank", "query"]);
}

/// The anchor value is passed through unchanged: the CLI never parses, trims,
/// or normalizes it, so the offset reaches the tool exactly as typed.
#[test]
fn recall_passes_the_anchor_offset_through_unchanged() {
    let args = recall_args(&["q", "--query-anchor", OFFSET_ANCHOR]).expect("build");
    assert_eq!(args["query_anchor"], json!(OFFSET_ANCHOR), "{args}");
}

/// `compose-context` binds the same key through its own flag list, and a call
/// without the flag sends no key at all.
#[test]
fn compose_context_query_anchor_sets_only_query_anchor() {
    let args =
        compose_args(&["q", "--budget-tokens", "512", "--query-anchor", ANCHOR]).expect("build");
    assert_eq!(args["query_anchor"], json!(ANCHOR), "{args}");
    assert_eq!(
        argument_keys(&args),
        ["budget_tokens", "query", "query_anchor"]
    );

    let without = compose_args(&["q", "--budget-tokens", "512"]).expect("build");
    assert!(
        !without
            .as_object()
            .expect("arguments object")
            .contains_key("query_anchor"),
        "absent, not null: {without}"
    );
}

/// A misspelled anchor flag is still an unknown flag on both commands, so the
/// parser fails closed with a usage error rather than ignoring it.
#[test]
fn a_misspelled_anchor_flag_is_still_a_usage_error() {
    let group = CoreGroup;
    let error = crate::cli_args::parse(
        &group.spec("recall"),
        tokens(&["q", "--query-anchors", ANCHOR]),
    )
    .expect_err("unknown flag must fail closed");
    assert_eq!(error.code(), "usage", "{}", error.message());
    assert!(
        error.message().contains("unknown flag"),
        "{}",
        error.message()
    );

    let group = WorkspaceGroup;
    let error = crate::cli_args::parse(
        &group.spec("compose-context"),
        tokens(&["--query-anchor-x", ANCHOR, "--budget-tokens", "512"]),
    )
    .expect_err("unknown flag must fail closed");
    assert_eq!(error.code(), "usage", "{}", error.message());
    assert!(
        error.message().contains("unknown flag"),
        "{}",
        error.message()
    );
}

/// Parse one command's JSON stdout.
fn json_of(text: &str) -> Value {
    serde_json::from_str(text.trim()).expect("json output")
}

/// Run one read verb against injected state with piped output.
fn run_state(verb: &str, args: &[&str], state: &Arc<McpState>) -> (ExitCode, String, String) {
    let env = |_key: &str| -> Option<String> { None };
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut input = std::io::empty();
    let code = {
        let mut io = crate::cli_read::CliIo {
            out: &mut out,
            err: &mut err,
            stdin: &mut input,
            is_tty: false,
            no_color: true,
            env: &env,
        };
        crate::cli_read::run_with_state(verb, tokens(args), state, &mut io)
    };
    let utf8 = |bytes: Vec<u8>| String::from_utf8(bytes).expect("utf8");
    (code, utf8(out), utf8(err))
}

/// Hermetic state with two in-window facts and lexical docs that never match
/// the query, so only the temporal arm can return them.
fn temporal_state() -> Arc<McpState> {
    let state = McpState::open_with_effective(
        &McpOpenOptions {
            default_bank: BANK.into(),
            ..McpOpenOptions::default()
        },
        &json!({}),
    )
    .expect("hermetic state");
    for (id, valid_from) in [
        ("tc-later", "2026-09-25T00:00:00Z"),
        ("tc-earlier", "2026-09-22T00:00:00Z"),
    ] {
        let entry = MemoryItem {
            id: id.into(),
            bank_id: BANK.into(),
            subject_id: "subj".into(),
            kind: ItemKind::Semantic,
            category: Some(SemanticCategory::ToolConfig),
            episodic_type: None,
            epistemic_kind: EpistemicKind::Fact,
            source_type: None,
            confidence: None,
            admission_score: Some(0.9),
            source_ref: Some("turn-1".into()),
            snapshot: Some(json!({ "entity": id, "amount": 1 })),
            gist: Some(format!("{id} memory note")),
            valid_from: valid_from.into(),
            valid_until: None,
            tx_from: "2024-01-01T00:00:00Z".into(),
            tx_until: None,
            created_at: "2024-01-01T00:00:00Z".into(),
            updated_at: "2024-01-01T00:00:00Z".into(),
            context: None,
        };
        state
            .store
            .create_memory_item(&entry, "agent")
            .expect("create item");
        state
            .index
            .put_lexical_doc(BANK, id, "unrelated")
            .expect("lexical doc");
    }
    state
}

/// Drop the wall-clock fields (latency counters and the fused score) so two
/// live calls of the same query can be compared key for key.
fn strip_volatile(mut value: Value) -> Value {
    value
        .as_object_mut()
        .expect("payload object")
        .remove("metrics");
    for hit in value["hits"].as_array_mut().expect("hits array") {
        let object = hit.as_object_mut().expect("hit object");
        object.remove("score");
        if let Some(scores) = object.get_mut("scores").and_then(Value::as_object_mut) {
            scores.remove("final");
        }
    }
    value
}

/// The anchored `recall` and the MCP `retrieve` dispatch produce the same
/// whole payload for the same state, apart from wall-clock fields.
#[test]
fn anchored_recall_matches_the_mcp_dispatch_payload() {
    let state = temporal_state();
    let mcp = dispatch(
        &state,
        "retrieve",
        &json!({"bank": BANK, "query": WINDOW_QUERY, "query_anchor": ANCHOR, "limit": 5}),
    );
    assert_eq!(mcp["ok"], json!(true), "MCP surface: {mcp}");
    let (code, out, err) = run_state(
        "recall",
        &[
            WINDOW_QUERY,
            "--query-anchor",
            ANCHOR,
            "--bank",
            BANK,
            "--limit",
            "5",
            "--output",
            "json",
        ],
        &state,
    );
    assert_eq!(code, ExitCode::from(0), "{err}");
    assert_eq!(json_of(&out)["ok"], json!(true), "CLI surface: {out}");
    // The CLI deliberately echoes `query` and the effective `limit` at the top
    // level for its text view; drop those echoes so the retrieval fields compare
    // key for key.
    let mut cli_payload = json_of(&out);
    let cli_object = cli_payload.as_object_mut().expect("payload");
    cli_object.remove("query");
    cli_object.remove("limit");
    assert_eq!(
        strip_volatile(cli_payload),
        strip_volatile(mcp),
        "CLI and MCP agree on the whole anchored payload"
    );
}

/// The anchored `compose-context` and the MCP `compose_context` dispatch
/// produce the same whole payload for the same state.
#[test]
fn anchored_compose_context_matches_the_mcp_dispatch_payload() {
    let state = temporal_state();
    let mcp = dispatch(
        &state,
        "compose_context",
        &json!({"bank": BANK, "query": WINDOW_QUERY, "query_anchor": ANCHOR, "budget_tokens": 512}),
    );
    assert_eq!(mcp["ok"], json!(true), "MCP surface: {mcp}");
    let (code, out, err) = run_state(
        "compose-context",
        &[
            WINDOW_QUERY,
            "--query-anchor",
            ANCHOR,
            "--bank",
            BANK,
            "--budget-tokens",
            "512",
            "--output",
            "json",
        ],
        &state,
    );
    assert_eq!(code, ExitCode::from(0), "{err}");
    assert_eq!(json_of(&out)["ok"], json!(true), "CLI surface: {out}");
    assert_eq!(
        json_of(&out),
        mcp,
        "CLI and MCP agree on the whole anchored compose payload"
    );
}

/// `CoreGroup::spec` answers each core verb with its registry record's parser
/// spec: the same value and boolean flag names, in the same order.
#[test]
fn core_group_specs_match_the_registered_records() {
    let expected: &[(&str, &[&'static str], &[&'static str])] = &[
        (
            "recall",
            &[
                "db",
                "backend",
                "bank",
                "actor",
                "output",
                "limit",
                "domains",
                "as-of",
                "query-anchor",
                "time-axis",
                "min-semantic",
                "min-keyword",
                "min-reranker",
                "min-final",
            ],
            &["no-input", "yes", "no-entities", "entity-summary"],
        ),
        (
            "get",
            &["db", "backend", "bank", "actor", "output"],
            &["no-input", "yes", "snapshot", "gist"],
        ),
        (
            "show",
            &["db", "backend", "bank", "actor", "output"],
            &["no-input", "yes", "snapshot", "gist"],
        ),
        (
            "inspect",
            &[
                "db",
                "backend",
                "bank",
                "actor",
                "output",
                "filter",
                "category",
                "epistemic-kind",
                "validity",
                "time-start",
                "time-end",
                "limit",
                "offset",
            ],
            &["no-input", "yes", "include-discarded"],
        ),
        (
            "stats",
            &["db", "backend", "bank", "actor", "output"],
            &["no-input", "yes"],
        ),
    ];
    for (verb, values, booleans) in expected {
        let spec = CoreGroup.spec(verb);
        assert_eq!(spec.value_flags(), *values, "{verb} value flags");
        assert_eq!(spec.bool_flags(), *booleans, "{verb} bool flags");
        let record =
            crate::cli_command_registry::find(verb).expect("every core verb is registered");
        assert_eq!(
            spec.value_flags(),
            record.parser_spec().value_flags(),
            "{verb}: the group must answer with the record"
        );
        assert_eq!(
            spec.bool_flags(),
            record.parser_spec().bool_flags(),
            "{verb}: the group must answer with the record"
        );
    }
}

/// A verb outside the group still gets the spec that accepts only the globals.
#[test]
fn an_unknown_verb_gets_the_empty_spec() {
    let spec = CoreGroup.spec("not-a-core-verb");
    assert_eq!(
        spec.value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(spec.bool_flags(), ["no-input", "yes"]);
}
