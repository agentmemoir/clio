// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the binding-level tests of mutator behaviour
//! that `mutator_tools` shapes itself: `correct` on belief and item targets,
//! `discard` confirmation, structured `update` values, the unknown-tool
//! fallback, and the `erase_request` confirmation gate.
//!
//! ## Owns
//! - Binding-level tests for `correct`, `discard`, `update` argument shaping,
//!   unknown-tool dispatch, and the `erase_request` confirmation gate
//!
//! ## Does not own
//! - Mutator semantics and storage (`clio-write`, `clio-compliance`, `clio-store`)
//! - Confirmed erasure effects, which `subject_request_tools` tests own
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the mutator argument and result shaping the
//! `mutator_tools` module performs before it calls an in-process tool.

use crate::runtime::{McpOpenOptions, McpState};
use crate::sync_tools::sync_tools_test_support_tests::TempDir;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

/// Dispatch one tool through the write surface, which routes mutator names to
/// `dispatch_inner` and the write names (`store`, `belief_observe`) to their
/// own handlers.
#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    crate::write_tools::dispatch(state, name, &args)
}

/// Store one item through the write surface and return its id.
fn store_item(state: &McpState, bank: &str, id: &str, gist: &str) {
    let out = call(
        state,
        "store",
        json!({
            "item": {"id": id, "subject_id": id, "gist": gist},
            "category": "tool_config",
            "epistemic_kind": "fact",
            "bank": bank
        }),
    );
    assert_eq!(out["ok"], json!(true), "store {id}: {out}");
}

/// Record one belief and return its id.
fn observe_belief(state: &McpState, bank: &str, proposition: &str) -> String {
    let out = call(
        state,
        "belief_observe",
        json!({
            "proposition": proposition,
            "confidence": 0.4,
            "evidence_ref": "turn_1",
            "source_type": "agent_inferred",
            "context": "morning standup",
            "bank": bank,
            "actor": "agent"
        }),
    );
    assert_eq!(out["outcome"], json!("created"), "{out}");
    out["belief"]["id"].as_str().expect("belief id").to_owned()
}

#[test]
fn an_unknown_mutator_name_is_not_implemented() {
    let s = state();
    let out = call(&s, "no_such_mutator", json!({}));
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("not_implemented"), "{out}");
}

#[test]
fn update_refuses_a_structured_new_value_instead_of_stringifying_it() {
    // Only a string or a number is a payload for `update`; anything else is
    // shaped to empty and refused by the in-process layer rather than stored.
    let s = state();
    store_item(&s, "bank-mcp-shape", "itm-shape-1", "shape target");
    let out = call(
        &s,
        "update",
        json!({
            "target": "itm-shape-1", "new_value": {"nested": true},
            "update_rule": "discrete", "bank": "bank-mcp-shape"
        }),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("invalid_argument"), "{out}");
}

#[test]
fn discard_reports_the_discarded_identity_only_after_confirmation() {
    let s = state();
    store_item(&s, "bank-mcp-discard", "itm-discard-1", "discard target");
    let confirmed = call(
        &s,
        "discard",
        json!({
            "item_id": "itm-discard-1", "reason": "operator cleanup",
            "confirm": true, "bank": "bank-mcp-discard"
        }),
    );
    assert_eq!(confirmed["ok"], json!(true), "{confirmed}");
    assert_eq!(confirmed["confirm"], json!(true), "{confirmed}");
    assert_eq!(confirmed["discarded"]["item_id"], json!("itm-discard-1"));
}

#[test]
fn correct_requires_a_reason_on_the_zero_write_preview() {
    let s = state();
    let out = call(
        &s,
        "correct",
        json!({
            "item_id": "itm-reason", "new_value": "v",
            "reason": "   ", "bank": "bank-mcp-reason"
        }),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("invalid_argument"), "{out}");
}

#[test]
fn correct_requires_a_new_value_for_an_item_target() {
    let s = state();
    let out = call(
        &s,
        "correct",
        json!({
            "item_id": "itm-value", "reason": "refresh",
            "bank": "bank-mcp-value"
        }),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("invalid_argument"), "{out}");
}

#[test]
fn correct_previews_a_belief_context_correction_without_writing() {
    let s = state();
    let bank = "bank-mcp-belief-preview";
    let belief = observe_belief(&s, bank, "The reviewer prefers diffs over prose");
    let preview = call(
        &s,
        "correct",
        json!({
            "item_id": belief, "reason": "fix source note",
            "context": "evening review", "bank": bank, "actor": "agent"
        }),
    );
    assert_eq!(preview["ok"], json!(true), "{preview}");
    assert_eq!(preview["confirm"], json!(false), "{preview}");
    assert_eq!(
        preview["would_correct"]["rule"],
        json!("belief_context"),
        "{preview}"
    );
    assert_eq!(preview["would_correct"]["belief_id"], json!(belief));
    assert_eq!(preview["would_correct"]["context_present"], json!(true));
}

#[test]
fn correct_confirms_a_belief_context_correction() {
    let s = state();
    let bank = "bank-mcp-belief-confirm";
    let belief = observe_belief(&s, bank, "The reviewer prefers short changelogs");
    let confirmed = call(
        &s,
        "correct",
        json!({
            "item_id": belief, "reason": "fix source note",
            "context": "evening review", "confirm": true,
            "bank": bank, "actor": "agent"
        }),
    );
    assert_eq!(confirmed["ok"], json!(true), "{confirmed}");
    assert_eq!(confirmed["rule"], json!("belief_context"), "{confirmed}");
    let history = crate::read_history::dispatch(
        &s,
        "belief_history",
        &json!({"belief_id_or_proposition": belief, "bank": bank}),
    )
    .expect("read tool routed")
    .expect("read succeeds");
    assert_eq!(history["context"], json!("evening review"), "{history}");
}

#[test]
fn correct_rejects_a_belief_request_that_carries_a_value() {
    let s = state();
    let bank = "bank-mcp-belief-value";
    let belief = observe_belief(&s, bank, "The reviewer prefers terse summaries");
    for confirm in [json!(false), json!(true)] {
        let rejected = call(
            &s,
            "correct",
            json!({
                "item_id": belief, "new_value": "0.9",
                "reason": "change value through correct",
                "context": "evening review", "confirm": confirm,
                "bank": bank, "actor": "agent"
            }),
        );
        assert_eq!(rejected["ok"], json!(false), "{rejected}");
        assert_eq!(rejected["code"], json!("invalid_argument"), "{rejected}");
        assert!(
            rejected["message"]
                .as_str()
                .unwrap_or_default()
                .contains("belief_observe"),
            "the rejection must name the only route for a belief value: {rejected}"
        );
    }
}

#[test]
fn correct_refuses_a_confirmed_belief_request_without_a_context() {
    let s = state();
    let bank = "bank-mcp-belief-nocontext";
    let belief = observe_belief(&s, bank, "The reviewer prefers short replies");
    let missing = call(
        &s,
        "correct",
        json!({
            "item_id": belief, "reason": "no context given", "confirm": true,
            "bank": bank, "actor": "agent"
        }),
    );
    assert_eq!(missing["ok"], json!(false), "{missing}");
    assert_eq!(missing["code"], json!("invalid_argument"), "{missing}");
}

#[test]
fn erase_request_never_mutates_without_confirmation() {
    // The confirmation gate sits ahead of every durable effect: an unconfirmed
    // call is refused and opens no subject request.
    let s = state();
    let out = call(
        &s,
        "erase_request",
        json!({
            "request_id": "req-mcp-unconfirmed",
            "subject_id": "subj-mcp-unconfirmed",
            "legal_basis": "GDPR_ART_17",
            "bank": "bank-mcp-erase",
            "confirm": false
        }),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("forbidden"), "{out}");
    assert!(
        s.durable
            .load("req-mcp-unconfirmed")
            .expect("load request")
            .is_none(),
        "an unconfirmed erase request must not be recorded"
    );
}

/// The marker that turns a re-executed copy of this binary into the child half
/// of the erase-gate check, carrying that child's scratch journal path.
const ERASE_GATE_CHILD_ENV: &str = "CLIO_MCP_ERASE_GATE_CHILD";

/// The line the child prints once its assertions held, so the parent can tell a
/// child that ran from a child the harness never selected.
const ERASE_GATE_CHILD_RAN: &str = "clio-mcp-erase-gate-child-ran";

/// This test's own path, so the child runs it and nothing else.
const ERASE_GATE_TEST_NAME: &str =
    "mutator_tools::mutator_tools_correct_tests::an_unconfirmed_erase_provisions_no_salt_file";

/// The salt override, cleared for the child: with no override the salt falls to
/// the sidecar, so a salt resolved ahead of the gate must leave that file behind.
const ERASE_GATE_SALT_ENV: &str = "CLIO_COMPLIANCE_SALT";

const ERASE_GATE_JOURNAL_ENV: &str = "CLIO_ERASURE_JOURNAL_FILE";

/// The confirm gate precedes salt provisioning: an unconfirmed `erase_request`
/// must not create `<journal>.salt`, which resolving the salt mints on first use.
///
/// The journal is the deployment resolution, read from the process environment,
/// so the call runs in a re-executed child that the parent hands a scratch
/// journal through that environment. Nothing here reads or mutates the handler's
/// own environment.
#[test]
fn an_unconfirmed_erase_provisions_no_salt_file() {
    match std::env::var_os(ERASE_GATE_CHILD_ENV) {
        Some(journal) => unconfirmed_erase_rejects_and_writes_nothing(Path::new(&journal)),
        None => run_erase_gate_child(),
    }
}

/// Hand the body a scratch journal its own process owns, and fail unless that
/// child actually ran it.
fn run_erase_gate_child() {
    let scratch = TempDir::new("mcp-erase-gate");
    let journal = scratch.key().with_file_name("erasure-journal.json");
    let output = Command::new(std::env::current_exe().expect("locate the test executable"))
        .args(["--exact", ERASE_GATE_TEST_NAME, "--nocapture"])
        .env(ERASE_GATE_CHILD_ENV, &journal)
        .env(ERASE_GATE_JOURNAL_ENV, &journal)
        .env_remove(ERASE_GATE_SALT_ENV)
        .output()
        .expect("run the erase-gate child");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains(ERASE_GATE_CHILD_RAN),
        "the erase-gate child did not report success:\n{stdout}\n{stderr}"
    );
}

/// The child half: one unconfirmed call, then the scratch directory must be as
/// empty as the gate left it.
fn unconfirmed_erase_rejects_and_writes_nothing(journal: &Path) {
    let s = state();
    let out = call(
        &s,
        "erase_request",
        json!({
            "request_id": "req-mcp-gate-salt",
            "subject_id": "subj-mcp-gate-salt",
            "legal_basis": "GDPR_ART_17",
            "bank": "bank-mcp-erase-gate",
            "confirm": false
        }),
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("forbidden"), "{out}");
    let mut salt = journal.as_os_str().to_os_string();
    salt.push(".salt");
    let salt = PathBuf::from(salt);
    assert!(
        !salt.exists(),
        "an unconfirmed erase provisioned {}",
        salt.display()
    );
    let created: Vec<_> = std::fs::read_dir(journal.parent().expect("the scratch directory"))
        .expect("read the scratch directory")
        .map(|entry| entry.expect("a scratch entry").file_name())
        .collect();
    assert!(
        created.is_empty(),
        "an unconfirmed erase created {created:?}"
    );
    println!("{ERASE_GATE_CHILD_RAN}");
}
