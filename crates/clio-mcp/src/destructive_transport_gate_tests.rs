// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the transport gate on the destructive tool
//! surfaces: which caller may run `repair`, `reindex`, or persist a retention
//! profile, and that the same surfaces answer a caller the transport has
//! vouched for.
//!
//! ## Owns
//! - The refusal an untrusted unbound state receives from `repair` and
//!   `reindex`, and the effect it leaves on the store
//! - The refusal the same state receives from a persisting
//!   `retention_profile_set`, and that its read-only preview is not gated
//! - The trusted unbound state those same calls succeed for
//!
//! ## Does not own
//! - The transport gate itself (see `runtime_transport_tests`)
//! - Operator scope and bank grants (see `ops_tools_grant_scope_tests`)
//! - The retention profile semantics (see `retention_tools_tests`)
//!
//! ## Boundary
//!
//! Tests here break when a destructive surface stops asking the transport gate;
//! they never change it.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::{Value, json};
use std::sync::Arc;

const TS: &str = "2026-01-01T00:00:00Z";

/// One in-memory state over a single `bank-a`, trusted as a local transport
/// opens it.
fn state() -> Arc<McpState> {
    McpState::open_with_effective(
        &McpOpenOptions {
            backend: "sqlite".into(),
            database_url: "sqlite::memory:".into(),
            default_bank: "bank-a".into(),
            default_actor: clio_types::Actor::Agent,
            shared_bank_enabled: false,
            kms_path: None,
        },
        &json!({}),
    )
    .expect("mcp state")
}

/// The same store as a transport that cannot vouch for its caller serves it.
fn untrusted() -> Arc<McpState> {
    let mut untrusted = state().as_ref().clone();
    untrusted.mark_network_untrusted();
    Arc::new(untrusted)
}

/// The refusal every gated surface answers with, asserted by code and text.
fn assert_transport_refusal(out: &Value, what: &str) {
    assert_eq!(out["ok"], Value::Bool(false), "{what}: {out}");
    assert_eq!(out["code"], "forbidden", "{what}: {out}");
    assert!(
        out["message"]
            .as_str()
            .is_some_and(|message| message.contains("unauthenticated")),
        "{what} names the transport rule: {out}"
    );
}

/// A live `repair` is a destructive verb, so a caller the transport cannot
/// vouch for never reaches the store with it.
#[test]
fn an_untrusted_state_cannot_run_a_confirmed_repair() {
    let out = dispatch(
        &untrusted(),
        "repair",
        &json!({
            "bank": "bank-a",
            "actions": ["drop_orphan_assoc_edge"],
            "confirm": true,
            "timestamp": TS
        }),
    );
    assert_transport_refusal(&out, "confirmed repair");
}

/// A live `reindex` rewrites the index for every bank in scope, so the same
/// gate refuses it before the sweep starts.
#[test]
fn an_untrusted_state_cannot_run_a_confirmed_reindex() {
    let out = dispatch(
        &untrusted(),
        "reindex",
        &json!({"bank": "bank-a", "target": "lexical", "confirm": true, "timestamp": TS}),
    );
    assert_transport_refusal(&out, "confirmed reindex");
}

/// The gate is the transport's and not the verb's: the same calls run for a
/// caller the transport vouches for.
#[test]
fn a_trusted_state_still_runs_the_destructive_verbs() {
    let st = state();
    let repair = dispatch(
        &st,
        "repair",
        &json!({
            "bank": "bank-a",
            "actions": ["drop_orphan_assoc_edge"],
            "confirm": true,
            "timestamp": TS
        }),
    );
    assert_eq!(repair["ok"], Value::Bool(true), "{repair}");
    let reindex = dispatch(
        &st,
        "reindex",
        &json!({"bank": "bank-a", "target": "lexical", "confirm": true, "timestamp": TS}),
    );
    assert_eq!(reindex["ok"], Value::Bool(true), "{reindex}");
}

/// The one call sits on the entry point every operator verb shares, so the
/// read-only arms are answered by the same gate: a caller the transport has not
/// vouched for reads no whole-store operator report either.
#[test]
fn an_untrusted_state_reads_no_whole_store_diagnose() {
    let out = dispatch(&untrusted(), "diagnose", &json!({"timestamp": TS}));
    assert_transport_refusal(&out, "diagnose");
}

/// Persisting a retention profile writes the store, so an unvouched caller is
/// refused at the write and not at the preview.
#[test]
fn an_untrusted_state_cannot_persist_a_retention_profile() {
    let profile = json!({"verbosity": "balanced"});
    let preview = dispatch(
        &untrusted(),
        "retention_profile_set",
        &json!({"bank": "bank-a", "profile": profile, "confirm": false}),
    );
    assert_eq!(
        preview["confirm"],
        Value::Bool(false),
        "the read-only preview is not gated: {preview}"
    );
    let out = dispatch(
        &untrusted(),
        "retention_profile_set",
        &json!({"bank": "bank-a", "profile": profile, "confirm": true}),
    );
    assert_transport_refusal(&out, "confirmed retention profile write");
}

/// The same write succeeds for a caller the transport vouches for.
#[test]
fn a_trusted_state_still_persists_a_retention_profile() {
    let out = dispatch(
        &state(),
        "retention_profile_set",
        &json!({
            "bank": "bank-a",
            "profile": {"verbosity": "balanced"},
            "confirm": true
        }),
    );
    assert_eq!(out["stored"], Value::Bool(true), "{out}");
}
