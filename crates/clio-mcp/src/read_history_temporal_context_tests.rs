// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP `temporal_history` context
//! response-shape tests: preference trajectories and belief trajectories must
//! surface the stored `context` and omit the key when there is none.
//!
//! ## Owns
//! - Serializer output-shape assertions for context presence/absence per
//!   temporal variant (preference points, belief outcome)
//!
//! ## Does not own
//! - Store-level persistence coverage (`clio-store` attribute/persona tests)
//! - Façade semantics (`clio-history` temporal tests)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::read_history;
use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::{Value, json};
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open(&McpOpenOptions::default()).expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn write(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

fn read(state: &McpState, name: &str, args: &Value) -> Value {
    read_history::dispatch(state, name, args)
        .expect("read tool routed")
        .expect("read succeeds")
}

#[test]
fn preference_temporal_history_surfaces_context_and_omits_when_absent() {
    // Updated from the previously-leaking behavior: the stored preference
    // context now rides only the points at or after the observation that
    // introduced it, so the earlier point omits the key.
    let state = state();
    write(
        &state,
        "persona_observe_preference",
        json!({
            "key": "punctuality",
            "observed_value": 0.4,
            "source": "explicit",
            "bank": "bank-a",
            "timestamp": "2026-01-01T00:00:00Z"
        }),
    );
    write(
        &state,
        "persona_observe_preference",
        json!({
            "key": "punctuality",
            "observed_value": 0.7,
            "source": "explicit",
            "context": "morning standups run long",
            "bank": "bank-a",
            "timestamp": "2026-02-01T00:00:00Z"
        }),
    );
    let traj = read(
        &state,
        "temporal_history",
        &json!({ "target": "preference:punctuality", "bank": "bank-a" }),
    );
    let points = traj["points"].as_array().expect("points array");
    assert_eq!(points.len(), 2, "{traj}");
    assert_eq!(points[0]["as_of"], "2026-01-01T00:00:00Z", "{traj}");
    assert!(
        points[0].get("context").is_none(),
        "the point before the context was introduced must omit the key: {traj}"
    );
    assert_eq!(
        points[1]["context"], "morning standups run long",
        "the effective point must carry the stored context: {traj}"
    );

    // Regression: an as_of that excludes the later point returns only the
    // earlier point, and its response omits the context key.
    let traj = read(
        &state,
        "temporal_history",
        &json!({
            "target": "preference:punctuality",
            "bank": "bank-a",
            "as_of": "2026-01-15T00:00:00Z"
        }),
    );
    let points = traj["points"].as_array().expect("points array");
    assert_eq!(points.len(), 1, "{traj}");
    let body = serde_json::to_string(&traj).expect("serialize trajectory");
    assert!(
        !body.contains("\"context\""),
        "a pre-context point-in-time read must omit the key: {traj}"
    );

    // A preference written without context omits the key on every point.
    write(
        &state,
        "persona_observe_preference",
        json!({
            "key": "chattiness",
            "observed_value": 0.5,
            "source": "explicit",
            "bank": "bank-a"
        }),
    );
    let traj = read(
        &state,
        "temporal_history",
        &json!({ "target": "preference:chattiness", "bank": "bank-a" }),
    );
    let body = serde_json::to_string(&traj).expect("serialize trajectory");
    assert!(
        !body.contains("\"context\""),
        "no-context preference must omit the key: {traj}"
    );
}

#[test]
fn belief_temporal_history_surfaces_context_and_omits_when_absent() {
    let state = state();
    write(
        &state,
        "belief_observe",
        json!({
            "proposition": "The indexer keeps lexical-first ordering",
            "confidence": 0.8,
            "evidence_ref": "turn_7",
            "source_type": "agent_inferred",
            "context": "design review",
            "bank": "bank-a"
        }),
    );
    let traj = read(
        &state,
        "temporal_history",
        &json!({
            "target": "belief:The indexer keeps lexical-first ordering",
            "bank": "bank-a"
        }),
    );
    assert_eq!(
        traj["context"], "design review",
        "belief context must surface on the trajectory: {traj}"
    );

    // A belief written without context omits the key entirely.
    write(
        &state,
        "belief_observe",
        json!({
            "proposition": "The scheduler prefers fairness",
            "confidence": 0.6,
            "evidence_ref": "turn_8",
            "source_type": "agent_inferred",
            "bank": "bank-a"
        }),
    );
    let traj = read(
        &state,
        "temporal_history",
        &json!({
            "target": "belief:The scheduler prefers fairness",
            "bank": "bank-a"
        }),
    );
    let body = serde_json::to_string(&traj).expect("serialize trajectory");
    assert!(
        !body.contains("\"context\""),
        "no-context belief must omit the key: {traj}"
    );
}
