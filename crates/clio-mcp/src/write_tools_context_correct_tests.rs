// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for MCP-surface tests of the optional `context`
//! argument on `correct`: explicit replacement on confirm, preservation when
//! absent, and presence-only echo on the unconfirmed preview.
//!
//! ## Owns
//! - `correct` replacement/preservation assertions over stored revisions
//! - Dry-run payload assertions (raw context never echoed)
//!
//! ## Does not own
//! - Plain `store`/`admit_preview` coverage (see `context_write_tests`)
//! - Correction routing semantics (see `clio-compliance` tests)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::{Value, json};
use std::sync::Arc;

const SECRET_CONTEXT: &str = "raw-context-must-never-be-echoed";

fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

fn store_with_context(state: &McpState, id: &str, context: &str) {
    let out = call(
        state,
        "store",
        json!({
            "item": {
                "id": id,
                "subject_id": format!("subj-{id}"),
                "gist": "original value",
                "snapshot": {"entity": "original value", "amount": 1, "date": "2024-01-01"}
            },
            "category": "persona",
            "epistemic_kind": "fact",
            "source_text": "The user records original value (1 note since 2024-01-01).",
            "bank": "bank-a",
            "actor": "agent",
            "context": context
        }),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
}

#[test]
fn correct_with_context_replaces_it_and_preview_hides_the_raw_value() {
    let s = state();
    store_with_context(&s, "itm-c1", "old context");

    let preview = call(
        &s,
        "correct",
        json!({
            "item_id": "itm-c1",
            "new_value": "corrected value",
            "reason": "fix typo",
            "context": SECRET_CONTEXT,
            "bank": "bank-a"
        }),
    );
    assert_eq!(preview["ok"], json!(true), "{preview}");
    assert_eq!(preview["confirm"], json!(false));
    assert_eq!(
        preview["would_correct"]["context_present"],
        json!(true),
        "{preview}"
    );
    let rendered = preview.to_string();
    assert!(
        !rendered.contains(SECRET_CONTEXT),
        "preview leaked the raw context: {preview}"
    );

    // The unconfirmed call wrote nothing: the prior revision is still open.
    let prior = s
        .store
        .get_memory_item("bank-a", "itm-c1")
        .expect("read back")
        .expect("stored");
    assert!(prior.tx_until.is_none());
    assert_eq!(prior.context.as_deref(), Some("old context"));

    let confirmed = call(
        &s,
        "correct",
        json!({
            "item_id": "itm-c1",
            "new_value": "corrected value",
            "reason": "fix typo",
            "context": SECRET_CONTEXT,
            "confirm": true,
            "bank": "bank-a"
        }),
    );
    assert_eq!(confirmed["ok"], json!(true), "{confirmed}");
    assert_eq!(confirmed["rule"], json!("discrete"));
    let new_id = confirmed["correction"]["new_item_id"]
        .as_str()
        .expect("corrected id");
    let corrected = s
        .store
        .get_memory_item("bank-a", new_id)
        .expect("read back")
        .expect("stored");
    assert_eq!(corrected.context.as_deref(), Some(SECRET_CONTEXT));
    assert_eq!(corrected.gist.as_deref(), Some("corrected value"));

    // The prior revision keeps its own context, closed not rewritten.
    let prior = s
        .store
        .get_memory_item("bank-a", "itm-c1")
        .expect("read back")
        .expect("stored");
    assert!(prior.tx_until.is_some());
    assert_eq!(prior.context.as_deref(), Some("old context"));
}

#[test]
fn correct_without_context_preserves_the_prior_context() {
    let s = state();
    store_with_context(&s, "itm-c2", "kept context");

    let preview = call(
        &s,
        "correct",
        json!({
            "item_id": "itm-c2",
            "new_value": "new value",
            "reason": "refresh",
            "bank": "bank-a"
        }),
    );
    assert_eq!(preview["would_correct"]["context_present"], json!(false));

    let confirmed = call(
        &s,
        "correct",
        json!({
            "item_id": "itm-c2",
            "new_value": "new value",
            "reason": "refresh",
            "confirm": true,
            "bank": "bank-a"
        }),
    );
    assert_eq!(confirmed["ok"], json!(true), "{confirmed}");
    let new_id = confirmed["correction"]["new_item_id"]
        .as_str()
        .expect("corrected id");
    let corrected = s
        .store
        .get_memory_item("bank-a", new_id)
        .expect("read back")
        .expect("stored");
    assert_eq!(corrected.context.as_deref(), Some("kept context"));
}

#[test]
fn correct_rejects_malformed_context_on_preview_and_confirm() {
    let s = state();
    store_with_context(&s, "itm-c3", "kept context");

    for (context, code) in [
        (json!(""), "invalid_argument"),
        (json!("a".repeat(4097)), "out_of_range"),
        (json!(7), "invalid_type"),
    ] {
        let mut preview_args = json!({
            "item_id": "itm-c3",
            "new_value": "new value",
            "reason": "refresh",
            "bank": "bank-a"
        });
        preview_args["context"] = context.clone();
        let preview = call(&s, "correct", preview_args);
        assert_eq!(preview["ok"], json!(false), "{preview}");
        assert_eq!(preview["code"], json!(code), "{preview}");

        let mut confirm_args = json!({
            "item_id": "itm-c3",
            "new_value": "new value",
            "reason": "refresh",
            "confirm": true,
            "bank": "bank-a"
        });
        confirm_args["context"] = context;
        let confirmed = call(&s, "correct", confirm_args);
        assert_eq!(confirmed["ok"], json!(false), "{confirmed}");
        assert_eq!(confirmed["code"], json!(code), "{confirmed}");
    }

    // Nothing was corrected: the original revision is still open and intact.
    let prior = s
        .store
        .get_memory_item("bank-a", "itm-c3")
        .expect("read back")
        .expect("stored");
    assert!(prior.tx_until.is_none());
    assert_eq!(prior.context.as_deref(), Some("kept context"));
}

#[test]
fn correct_routes_belief_targets_to_the_authorized_context_correction() {
    let s = state();
    let observed = call(
        &s,
        "belief_observe",
        json!({
            "proposition": "The reviewer prefers diffs over prose",
            "confidence": 0.4,
            "evidence_ref": "turn_1",
            "source_type": "agent_inferred",
            "context": "morning standup",
            "bank": "bank-a",
            "actor": "agent"
        }),
    );
    assert_eq!(observed["outcome"], json!("created"), "{observed}");
    let belief_id = observed["belief"]["id"].as_str().expect("belief id");

    // The preview resolves the belief target and never exposes raw values. A
    // context-only request (no `new_value`) is the sole route to a belief.
    let preview = call(
        &s,
        "correct",
        json!({
            "item_id": belief_id,
            "reason": "fix source note",
            "context": "evening review",
            "bank": "bank-a",
            "actor": "agent"
        }),
    );
    assert_eq!(
        preview["would_correct"]["rule"],
        json!("belief_context"),
        "{preview}"
    );
    assert_eq!(
        preview["would_correct"]["belief_id"],
        json!(belief_id),
        "{preview}"
    );
    assert_eq!(
        preview["would_correct"]["context_present"],
        json!(true),
        "{preview}"
    );

    let confirmed = call(
        &s,
        "correct",
        json!({
            "item_id": belief_id,
            "reason": "fix source note",
            "context": "evening review",
            "confirm": true,
            "bank": "bank-a",
            "actor": "agent"
        }),
    );
    assert_eq!(confirmed["rule"], json!("belief_context"), "{confirmed}");

    // The stored belief-level context is the corrected value. The prior value
    // is not recoverable from the correction audit row, which keeps only
    // hash/presence/length stats.
    let history = crate::read_history::dispatch(
        &s,
        "belief_history",
        &json!({"belief_id_or_proposition": belief_id, "bank": "bank-a"}),
    )
    .expect("read tool routed")
    .expect("read succeeds");
    assert_eq!(history["context"], json!("evening review"), "{history}");

    // Belief context corrections require a context; a context-less request on a
    // belief target stays rejected (values evolve via belief_observe).
    let missing = call(
        &s,
        "correct",
        json!({
            "item_id": belief_id,
            "reason": "no context given",
            "confirm": true,
            "bank": "bank-a",
            "actor": "agent"
        }),
    );
    assert_eq!(missing["ok"], json!(false), "{missing}");
    assert_eq!(missing["code"], json!("invalid_argument"), "{missing}");
}

#[test]
fn correct_belief_target_with_only_a_value_change_stays_rejected() {
    let s = state();
    let observed = call(
        &s,
        "belief_observe",
        json!({
            "proposition": "The reviewer prefers terse summaries",
            "confidence": 0.4,
            "evidence_ref": "turn_1",
            "source_type": "agent_inferred",
            "context": "standup",
            "bank": "bank-a",
            "actor": "agent"
        }),
    );
    assert_eq!(observed["outcome"], json!("created"), "{observed}");
    let belief_id = observed["belief"]["id"].as_str().expect("belief id");
    let rejected = call(
        &s,
        "correct",
        json!({
            "item_id": belief_id,
            "new_value": "0.9",
            "reason": "try to change confidence",
            "confirm": true,
            "bank": "bank-a",
            "actor": "agent"
        }),
    );
    // The belief-context correction requires a supplied context; a bare
    // value correction is rejected as a usage error.
    assert_eq!(rejected["ok"], json!(false), "{rejected}");
    assert_eq!(rejected["code"], json!("invalid_argument"), "{rejected}");
}

/// A belief `correct` request that carries a non-empty `new_value` alongside a
/// `context` must fail closed: a belief's values/confidence evolve only through
/// `belief_observe`, and the context-only route must not silently drop the
/// value request. Nothing is stored by the rejected request.
#[test]
fn correct_belief_rejects_a_combined_context_and_value_request() {
    let s = state();
    let observed = call(
        &s,
        "belief_observe",
        json!({
            "proposition": "The reviewer prefers short changelogs",
            "confidence": 0.4,
            "evidence_ref": "turn_1",
            "source_type": "agent_inferred",
            "context": "standup",
            "bank": "bank-a",
            "actor": "agent"
        }),
    );
    assert_eq!(observed["outcome"], json!("created"), "{observed}");
    let belief_id = observed["belief"]["id"].as_str().expect("belief id");

    for confirm in [json!(false), json!(true)] {
        let rejected = call(
            &s,
            "correct",
            json!({
                "item_id": belief_id,
                "new_value": "0.9",
                "reason": "change value and context together",
                "context": "evening review",
                "confirm": confirm,
                "bank": "bank-a",
                "actor": "agent"
            }),
        );
        assert_eq!(rejected["ok"], json!(false), "{rejected}");
        assert_eq!(rejected["code"], json!("invalid_argument"), "{rejected}");
        let message = rejected["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("belief_observe"),
            "the rejection must point values/confidences at belief_observe: {rejected}"
        );
    }

    // The rejected request stored nothing: context unchanged, no new entry.
    let history = crate::read_history::dispatch(
        &s,
        "belief_history",
        &json!({"belief_id_or_proposition": belief_id, "bank": "bank-a"}),
    )
    .expect("read tool routed")
    .expect("read succeeds");
    assert_eq!(history["context"], json!("standup"), "{history}");
    assert_eq!(
        history["entries"].as_array().map(Vec::len),
        Some(1),
        "{history}"
    );
}
