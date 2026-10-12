// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the intent, context-composition, and audit read commands
//! (`intent gate`, `compose-context`, `audit trail`), including audit masking.
//!
//! ## Owns
//! - Assertions over the workspace group's builds, masking, and text views
//!
//! ## Does not own
//! - The command implementation under test (`cli_read_workspace`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{WorkspaceGroup, render_audit, render_compose, render_gate};
use crate::cli_args::ParsedArgs;
use crate::cli_read_group::ReadGroup;
use crate::cli_read_test_support::{TS, call, json_of, run_state, seed_item, state};
use clio_mcp::runtime::McpState;
use serde_json::json;
use std::process::ExitCode;
use std::sync::Arc;

fn workspace_state() -> Arc<McpState> {
    let s = state();
    seed_item(&s, "bank-a", "itm-1", "terse answers");
    // The secret rides a declared leaf on purpose: an undeclared snapshot
    // leaf fails span verification and would be refused instead of stored,
    // so this fixture proves masking for a secret that was admitted.
    let planted = call(
        &s,
        "store",
        json!({
            "item": {
                "id": "itm-secret",
                "subject_id": "s",
                "gist": "rotate keys",
                "snapshot": {
                    "entity": "sk-live-supersecret", "amount": 1, "date": "2024-01-01"
                }
            },
            "category": "task_spec",
            "epistemic_kind": "fact",
            "source_text": "The user prefers creds (1 note since 2024-01-01) sk-live-supersecret.",
            "bank": "bank-a",
            "actor": "agent",
            "timestamp": TS
        }),
    );
    assert_eq!(planted["ok"], json!(true), "{planted}");
    s
}

#[test]
fn intent_gate_success_and_text() {
    let s = workspace_state();
    let (code, out, err) = run_state(
        "intent gate",
        &["deploy status", "--bank", "bank-a"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(0));
    assert!(json_of(&out)["retrieval_needed"].is_boolean());
    assert!(err.is_empty(), "{err}");

    let (_, out, _) = run_state(
        "intent gate",
        &["deploy status", "--bank", "bank-a"],
        &s,
        true,
    );
    assert!(out.contains("retrieval_needed:"), "{out}");

    assert_eq!(
        render_gate(
            &json!({"retrieval_needed": false, "domains": ["semantic"], "confidence": 0.5, "reason": "chat"})
        ),
        "retrieval_needed: false\ndomains: semantic\nconfidence: 0.5\nreason: chat"
    );
    assert_eq!(render_gate(&json!({})), "retrieval_needed: false");
}

#[test]
fn compose_context_requires_budget_and_renders() {
    let s = workspace_state();
    let (code, out, _) = run_state(
        "compose-context",
        &["--budget-tokens", "2000", "--bank", "bank-a"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(0));
    let value = json_of(&out);
    assert_eq!(value["budget_tokens"], json!(2000));

    let (_, out, _) = run_state(
        "compose-context",
        &[
            "terse",
            "--budget-tokens",
            "2000",
            "--bank",
            "bank-a",
            "--domains",
            "semantic,episodic",
            "--as-of",
            "2030-01-01T00:00:00Z",
            "--time-axis",
            "valid",
            "--recall-scope",
            "full",
            "--expand-graph",
            "--prefer-consolidated",
        ],
        &s,
        true,
    );
    assert!(out.contains("sections:"), "{out}");
    assert!(out.contains("budget_tokens: 2000"), "{out}");

    assert_eq!(
        render_compose(
            &json!({"sections": [{}], "total_tokens": 5, "budget_tokens": 9, "truncated_ids": ["a"]})
        ),
        "sections: 1\ntotal_tokens: 5\nbudget_tokens: 9\ntruncated_ids: 1"
    );
    assert_eq!(
        render_compose(&json!({})),
        "sections: 0\ntotal_tokens: 0\nbudget_tokens: 0\ntruncated_ids: 0"
    );
}

#[test]
fn compose_context_usage_errors() {
    let s = workspace_state();
    let cases: &[(&[&str], &str)] = &[
        (&["--bank", "bank-a"], "--budget-tokens"),
        (&["a", "b", "--budget-tokens", "1"], "unexpected extra"),
        (&["--budget-tokens", "x"], "non-negative integer"),
        (
            &["--budget-tokens", "1", "--recall-scope", "bogus"],
            "--recall-scope",
        ),
        (
            &["--budget-tokens", "1", "--time-axis", "bogus"],
            "--time-axis",
        ),
    ];
    for &(args, needle) in cases {
        let (code, out, err) = run_state("compose-context", args, &s, false);
        assert_eq!(code, ExitCode::from(2), "{args:?}");
        assert!(out.is_empty(), "{args:?}: {out}");
        assert!(err.contains(needle), "{args:?}: {err}");
    }
}

#[test]
fn audit_trail_masks_planted_secret() {
    let s = workspace_state();
    let (code, out, err) = run_state(
        "audit trail",
        &["itm-secret", "--bank", "bank-a"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(0));
    assert!(err.is_empty(), "{err}");
    assert!(!out.contains("supersecret"), "secret leaked: {out}");
    assert_eq!(json_of(&out)["item_id"], json!("itm-secret"));

    // The audit view never echoes item payloads, so the planted secret cannot
    // leak; the CLI additionally applies its own defensive mask pass, which a
    // synthetic secret-keyed field proves.
    let mut payload = json!({
        "item_id": "i",
        "revisions": [{"api_key": "sk-live-supersecret"}],
    });
    WorkspaceGroup.decorate(
        "audit trail",
        &ParsedArgs::default(),
        &serde_json::json!({}),
        &mut payload,
    );
    let masked = payload.to_string();
    assert!(!masked.contains("supersecret"), "{masked}");
    assert!(
        masked.contains("****") || masked.contains("[REDACTED]"),
        "{masked}"
    );

    let (_, out, _) = run_state("audit trail", &["itm-secret", "--bank", "bank-a"], &s, true);
    assert!(!out.contains("supersecret"), "secret leaked: {out}");
    assert!(out.contains("item_id: itm-secret"), "{out}");

    // The mask must not rewrite non-secret numeric counters whose key names
    // merely contain a marker substring, or the view would drift from the tool.
    let mut numeric = json!({
        "item_id": "i",
        "revisions": [{"api_key": "sk-live-supersecret", "token_count": 3}],
    });
    WorkspaceGroup.decorate(
        "audit trail",
        &ParsedArgs::default(),
        &serde_json::json!({}),
        &mut numeric,
    );
    assert_eq!(numeric["revisions"][0]["api_key"], json!("****cret"));
    assert_eq!(numeric["revisions"][0]["token_count"], json!(3));

    assert_eq!(
        render_audit(
            &json!({"item_id": "i", "lineage_ids": ["a"], "revisions": [], "events": [], "timeline": []})
        ),
        "item_id: i\nlineage_ids: 1\nrevisions: 0\nevents: 0\ntimeline: 0"
    );
    assert_eq!(
        render_audit(&json!({})),
        "item_id: ?\nlineage_ids: 0\nrevisions: 0\nevents: 0\ntimeline: 0"
    );
}

#[test]
fn audit_trail_and_intent_gate_usage_errors() {
    let s = workspace_state();
    for verb in ["intent gate", "audit trail"] {
        let (code, out, err) = run_state(verb, &["--bank", "bank-a"], &s, false);
        assert_eq!(code, ExitCode::from(2), "{verb}");
        assert!(out.is_empty(), "{verb}: {out}");
        assert!(err.contains("requires a"), "{verb}: {err}");
    }
}

/// Every verb's spec is its record's, and the accepted set that record carries is
/// the one the group's own list held: same flags, same order, globals included.
#[test]
fn spec_answers_from_the_registry_with_the_original_accepted_set() {
    let expected: &[(&str, &[&str], &[&str])] = &[
        (
            "intent gate",
            &["db", "backend", "bank", "actor", "output"],
            &["no-input", "yes", "explain"],
        ),
        (
            "compose-context",
            &[
                "db",
                "backend",
                "bank",
                "actor",
                "output",
                "query",
                "domains",
                "budget-tokens",
                "as-of",
                "query-anchor",
                "time-axis",
                "recall-scope",
            ],
            &[
                "no-input",
                "yes",
                "expand-graph",
                "prefer-consolidated",
                "explain",
            ],
        ),
        (
            "audit trail",
            &["db", "backend", "bank", "actor", "output"],
            &["no-input", "yes", "explain"],
        ),
    ];
    let group = WorkspaceGroup;
    assert_eq!(group.verbs().len(), expected.len(), "every verb is covered");
    for &(verb, value, boolean) in expected {
        let spec = group.spec(verb);
        assert_eq!(spec.value_flags(), value, "{verb} value flags");
        assert_eq!(spec.bool_flags(), boolean, "{verb} bool flags");
        let record = crate::cli_command_registry::find(verb).expect("a record");
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
