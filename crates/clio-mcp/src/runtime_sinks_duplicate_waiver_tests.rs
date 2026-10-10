// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! Tests that the unconditional exact-duplicate rejection fires on every gated
//! write surface and that every one of them can waive it.
//!
//! ## Owns
//! - The default profile refusing an identical re-write, and `allow_duplicate` admitting it
//! - A non-`store` surface refusing, then admitting under the same waiver
//! - A strict bank's rejection reason staying the unconditional one
//!
//! ## Does not own
//! - Neighbour discovery and the signal source (see `runtime_sinks_tests`)
//! - Novelty arithmetic and force semantics (see `clio-admission`)
//!
//! ## Boundary
//!
//! Changes to the duplicate-rejection contract or its waiver should normally
//! require changes here. Changes to signal lookup should not require changes here.
//!
//! Keep this module focused on the duplicate-rejection write contract.

use crate::runtime::{McpOpenOptions, McpState};
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open(&McpOpenOptions::default()).expect("state")
}

#[test]
fn default_profile_rejects_an_exact_duplicate_and_the_waiver_admits_it() {
    let s = state();
    let write = |bank: &str, id: &str, allow_duplicate: bool| {
        let mut args = serde_json::json!({
            "item": {"id": id, "gist": "Satoshi lives in Japan"},
            "category": "persona",
            "epistemic_kind": "fact",
            "bank": bank,
            "actor": "agent"
        });
        if allow_duplicate {
            args["allow_duplicate"] = serde_json::json!(true);
        }
        crate::write_tools::dispatch(&s, "store", &args)
    };

    let first = write("bank-default-dedup", "itm-d1", false);
    assert_eq!(first["pass"], serde_json::json!(true), "{first}");

    let second = write("bank-default-dedup", "itm-d2", false);
    assert_eq!(second["ok"], serde_json::json!(false), "{second}");
    assert_eq!(second["code"], serde_json::json!("duplicate"), "{second}");
    assert_eq!(
        second["factors"]["novelty"],
        serde_json::json!(0.0),
        "{second}"
    );
    assert_eq!(second["pass"], serde_json::json!(false), "{second}");
    assert_eq!(second["duplicate"], serde_json::json!(true), "{second}");
    assert!(
        second["rejection_reason"]
            .as_str()
            .unwrap_or_default()
            .starts_with("exact_duplicate:"),
        "{second}"
    );
    assert!(second.get("id").is_none_or(serde_json::Value::is_null));

    let forced = write("bank-default-dedup", "itm-d3", true);
    assert_eq!(forced["pass"], serde_json::json!(true), "{forced}");
    assert!(forced["id"].is_string(), "{forced}");
}

#[test]
fn strict_bank_rejects_exact_duplicate_at_write_time() {
    let s = state();
    let bank = "bank-strict-dedup";
    let set = crate::write_tools::dispatch(
        &s,
        "retention_profile_set",
        &serde_json::json!({
            "bank": bank,
            "profile": {"duplicate_tolerance_write": "strict"},
            "confirm": true
        }),
    );
    assert_eq!(set["stored"], serde_json::json!(true), "{set}");
    let write = |id: &str| {
        crate::write_tools::dispatch(
            &s,
            "store",
            &serde_json::json!({
                "item": {"id": id, "gist": "Satoshi lives in Japan"},
                "category": "persona",
                "epistemic_kind": "fact",
                "bank": bank,
                "actor": "agent"
            }),
        )
    };
    let first = write("itm-dup-1");
    assert_eq!(first["pass"], serde_json::json!(true), "{first}");
    assert_eq!(
        first["factors"]["novelty"],
        serde_json::json!(1.0),
        "{first}"
    );
    // The exact-duplicate rule now rejects this before the strict floor is
    // even consulted, so the reason is the unconditional one. A near-duplicate
    // is what the strict floor is for, and it is covered in `clio-admission`.
    let second = write("itm-dup-2");
    assert_eq!(second["ok"], serde_json::json!(false), "{second}");
    assert_eq!(second["code"], serde_json::json!("duplicate"), "{second}");
    assert_eq!(second["pass"], serde_json::json!(false), "{second}");
    assert_eq!(
        second["factors"]["novelty"],
        serde_json::json!(0.0),
        "{second}"
    );
    assert!(
        second["rejection_reason"]
            .as_str()
            .unwrap_or_default()
            .starts_with("exact_duplicate:"),
        "{second}"
    );
    assert!(second.get("id").is_none_or(serde_json::Value::is_null));
}

/// Every gated write surface must be able to waive the duplicate rejection, not
/// just `store`. An identical re-write used to be accepted everywhere; after the
/// rule became unconditional, a surface without a waiver would have become
/// permanently un-writable with no recourse.
#[test]
fn a_non_store_surface_can_waive_the_duplicate_rejection() {
    let s = state();
    let bank = "bank-triple-waiver";
    // `triple_add` derives edge identity from subject/predicate/object, so
    // there is no id to pass; the three calls below differ only in the waiver.
    let add = |allow_duplicate: bool| {
        let mut args = serde_json::json!({
            "subject": "Alice", "predicate": "works_at", "object": "Acme",
            "epistemic_kind": "fact", "snapshot": {"entity": "Acme"},
            "source_text": "Alice works at Acme.",
            "bank": bank, "actor": "agent", "timestamp": "2024-01-01T00:00:00Z"
        });
        if allow_duplicate {
            args["allow_duplicate"] = serde_json::json!(true);
        }
        crate::write_tools::dispatch(&s, "triple_add", &args)
    };

    let first = add(false);
    assert_eq!(first["ok"], serde_json::json!(true), "{first}");

    // `triple_add` nests its decision under `decision`, unlike `store`.
    let repeat = add(false);
    assert_eq!(
        repeat["decision"]["pass"],
        serde_json::json!(false),
        "{repeat}"
    );
    assert!(
        repeat["decision"]["rejection_reason"]
            .as_str()
            .unwrap_or_default()
            .starts_with("exact_duplicate:"),
        "{repeat}"
    );

    let waived = add(true);
    assert_eq!(waived["ok"], serde_json::json!(true), "{waived}");
    assert_eq!(
        waived["decision"]["pass"],
        serde_json::json!(true),
        "{waived}"
    );
    assert!(waived["new_edge_id"].is_string(), "{waived}");
}
