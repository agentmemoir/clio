// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared read-conformance fixture state used
//! by the split integration-test binaries: the fixture bank/timestamp constants
//! and the seeded `McpState` builders.
//!
//! ## Owns
//! - `BANK` / `TS`, the fixture bank and its deterministic timestamp
//! - `fresh_state`, the unseeded SQLite runtime state
//! - `seed`, the canonical read fixture item and write set
//! - `read_state`, the seeded state shared by both read-conformance binaries
//!
//! ## Does not own
//! - Transport framing or parity masking (each binary owns its own harness)
//! - Direct-dispatch or transport assertions (the test modules own those)
//!
//! ## Boundary
//!
//! Changes to the shared read fixture data should normally require changes here.
//! Changes to a single binary's transport harness should not require changes here.
//!
//! Keep this module focused on the shared read-conformance fixture state.
//
// The same module is included by two integration-test binaries. A helper that
// only one of them references must not fail the build, so cross-binary dead
// code is allowed here rather than deleting or duplicating a shared fixture.
#![allow(dead_code)]

use clio_mcp::runtime::{McpOpenOptions, McpState};
use serde_json::{Value, json};
use std::sync::Arc;

/// Fixture bank for the read conformance suites.
pub const BANK: &str = "read-conf";
/// Deterministic fixture timestamp for seeded writes.
pub const TS: &str = "2024-01-01T00:00:00Z";

/// Unseeded SQLite runtime state on the fixture bank.
pub fn fresh_state() -> Arc<McpState> {
    McpState::open(&McpOpenOptions {
        default_bank: BANK.into(),
        ..McpOpenOptions::default()
    })
    .expect("state")
}

/// Seed the canonical read fixture: three stored items plus persona, task,
/// failure, triple, belief, and graph rows.
pub fn seed(state: &McpState) {
    // Each gist carries its own id. Two items sharing one gist are exact
    // duplicates, and the unconditional write-side rule refuses the second one,
    // so the fixture would not seed three rows.
    for (id, category, gist) in [
        ("cf-1", "task_spec", "terse answers cf-1"),
        ("cf-2", "persona", "terse answers cf-2"),
        ("cf-3", "task_spec", "verbose replies cf-3"),
    ] {
        let out = clio_mcp::write_tools::dispatch(
            state,
            "store",
            &json!({
                "item": {"id": id, "subject_id": "subj", "gist": gist,
                         "snapshot": {"entity": gist, "amount": 1, "date": "2024-01-01"}},
                "category": category, "epistemic_kind": "fact",
                "source_text": format!("The user prefers {gist} (1 note since 2024-01-01)."),
                "bank": BANK, "actor": "agent", "timestamp": TS
            }),
        );
        assert_eq!(out["ok"], json!(true), "seed store {id}: {out}");
        state
            .index
            .put_lexical_doc(BANK, id, gist)
            .expect("lexical doc");
    }
    let writes: Vec<(&str, Value)> = vec![
        (
            "persona_put_stable",
            json!({"key": "style", "value": "concise", "timestamp": TS}),
        ),
        (
            "task_upsert",
            json!({"task_id": "t-1", "definition": "Run conformance", "status": "running", "timestamp": TS}),
        ),
        (
            "failure_record",
            json!({"task_id": "t-1", "attempt_n": 1, "what_failed": "flake", "lesson": "retry once", "timestamp": TS}),
        ),
        (
            "triple_add",
            json!({"subject": "rust", "predicate": "used_by", "object": "agent", "epistemic_kind": "fact", "valid_from": TS, "timestamp": TS}),
        ),
        (
            "belief_observe",
            json!({"proposition": "Friday deploys are risky", "confidence": 0.7, "evidence_ref": "ev-1", "source_type": "user_stated", "timestamp": TS}),
        ),
        (
            "graph_link",
            json!({"source_id": "cf-1", "target_id": "cf-2", "relationship": "relates_to"}),
        ),
    ];
    for (name, mut args) in writes {
        args["bank"] = json!(BANK);
        args["actor"] = json!("agent");
        let out = clio_mcp::write_tools::dispatch(state, name, &args);
        assert_eq!(out["ok"], json!(true), "seed {name}: {out}");
    }
}

/// Seeded runtime state shared by both read-conformance binaries.
pub fn read_state() -> Arc<McpState> {
    let state = fresh_state();
    seed(&state);
    state
}
