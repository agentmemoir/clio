// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that the read-surface item payloads state an item's stored
//! lifecycle markers, including the archive marker set through the real
//! cleanup path.
//!
//! ## Owns
//! - The archive marker reaching `get` after a confirmed `hygiene_clean`
//! - The lifecycle flags on the snapshot and gist read modes
//!
//! ## Does not own
//! - Where the flags are stamped (`read_item_lifecycle`)
//! - The discard write path or the inspect listing (`read_tools_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the two lifecycle markers on the item reads.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::{Value, json};
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

/// Store one admitted item in `bank-a`.
fn store_item(state: &McpState, id: &str, text: &str) {
    let args = json!({
        "item": {
            "id": id,
            "subject_id": "subj",
            "gist": text,
            "snapshot": {"entity": text, "amount": 1, "date": "2024-01-01"}
        },
        "category": "task_spec",
        "epistemic_kind": "fact",
        "source_text": format!("The user prefers {text} (1 note since 2024-01-01)."),
        "bank": "bank-a",
        "actor": "agent",
        "timestamp": "2024-01-01T00:00:00Z"
    });
    let out = call(state, "store", args);
    assert_eq!(out["ok"], json!(true), "store {id}: {out}");
}

#[test]
fn archiving_through_cleanup_makes_get_report_archived() {
    let s = state();
    store_item(&s, "itm-arc", "archivable note");

    let live = call(&s, "get", json!({"bank": "bank-a", "item_id": "itm-arc"}));
    assert_eq!(live["archived"], json!(false), "{live}");
    assert_eq!(live["discarded"], json!(false), "{live}");

    let cleaned = call(
        &s,
        "hygiene_clean",
        json!({
            "bank": "bank-a",
            "item_ids": ["itm-arc"],
            "action": "archive",
            "confirm": true,
            "timestamp": "2024-03-01T00:00:00Z"
        }),
    );
    assert_eq!(cleaned["ok"], json!(true), "{cleaned}");

    let archived = call(&s, "get", json!({"bank": "bank-a", "item_id": "itm-arc"}));
    assert_eq!(archived["ok"], json!(true), "{archived}");
    assert_eq!(archived["archived"], json!(true), "{archived}");
    assert_eq!(archived["discarded"], json!(false), "{archived}");
}

#[test]
fn snapshot_and_gist_report_the_lifecycle_markers() {
    let s = state();
    store_item(&s, "itm-live", "live note");
    store_item(&s, "itm-gone", "gone note");

    let discarded = call(
        &s,
        "discard",
        json!({
            "bank": "bank-a",
            "item_id": "itm-gone",
            "reason": "ops",
            "confirm": true,
            "timestamp": "2024-02-01T00:00:00Z"
        }),
    );
    assert_eq!(discarded["ok"], json!(true), "{discarded}");

    for (mode, id, flagged) in [
        ("get_snapshot", "itm-live", false),
        ("get_snapshot", "itm-gone", true),
        ("get_gist", "itm-live", false),
        ("get_gist", "itm-gone", true),
    ] {
        let out = call(&s, mode, json!({"bank": "bank-a", "item_id": id}));
        assert_eq!(out["ok"], json!(true), "{mode} {id}: {out}");
        assert_eq!(out["discarded"], json!(flagged), "{mode} {id}: {out}");
        assert_eq!(out["archived"], json!(false), "{mode} {id}: {out}");
    }
}
