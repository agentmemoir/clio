// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the scope each of the five operator tools reaches for a
//! bound MCP caller: the bank it reports, the bank it names, the foreign bank it
//! refuses, and the foreign state a refusal leaves behind.
//!
//! ## Owns
//! - The per-tool scope assertions for a caller that omits `bank`, and the
//!   refusal a caller that names a bank outside its grant receives
//! - The foreign bank's edge, item, and index state after a refused call
//! - The unbound snapshots those scopes are compared against
//!
//! ## Does not own
//! - The plan and doctor-note cases (`ops_tools_grant_tests`)
//! - The fixtures themselves (`ops_tools_grant_tests`)
//! - Operator semantics themselves (`clio-ops`)
//!
//! ## Boundary
//!
//! Tests here break when an operator arm changes the scope it resolves, forwards,
//! or refuses; they never change it.
//!
//! Keep this module focused on the scope one operator call reaches.

use super::ops_tools_grant_tests::{
    TS, bound, count_items, has_finding, index_rows, plant_orphan_edge, seed_item, state,
};
use crate::runtime::McpState;
use crate::write_tools::dispatch;
use serde_json::{Value, json};
use std::sync::Arc;

#[test]
fn bound_diagnose_does_not_disclose_a_foreign_bank() {
    let original = state(false);
    seed_item(&original, "bank-a", "itm-a");
    seed_item(&original, "bank-b", "itm-b");
    let restricted = bound(&original, &["bank-a"]);

    let scoped = dispatch(&restricted, "diagnose", &json!({ "timestamp": TS }));
    assert_eq!(scoped["ok"], Value::Bool(true), "{scoped}");
    assert_eq!(sorted_report_banks(&scoped), ["bank-a"], "{scoped}");
    assert_eq!(scoped["indexable_items"], 1, "{scoped}");

    let whole = dispatch(&original, "diagnose", &json!({ "timestamp": TS }));
    assert_eq!(sorted_report_banks(&whole), ["bank-a", "bank-b"], "{whole}");
    assert_eq!(whole["indexable_items"], 2, "{whole}");
}

#[test]
fn bound_diagnose_with_an_explicit_foreign_bank_is_forbidden() {
    let original = two_bank_store();
    let restricted = bound(&original, &["bank-a"]);

    let out = dispatch(
        &restricted,
        "diagnose",
        &json!({ "bank": "bank-b", "timestamp": TS }),
    );
    assert_eq!(out["ok"], Value::Bool(false), "{out}");
    assert_eq!(out["code"], "forbidden", "{out}");
    assert!(
        out["banks"].is_null(),
        "a refused diagnose reported banks: {out}"
    );

    // The same request from the unbound state is served, so the refusal above is
    // the grant rather than a broken argument.
    let open = dispatch(
        &original,
        "diagnose",
        &json!({ "bank": "bank-b", "timestamp": TS }),
    );
    assert_eq!(open["ok"], Value::Bool(true), "{open}");
}

#[test]
fn bound_verify_with_an_explicit_foreign_bank_is_forbidden() {
    let original = two_bank_store();
    let restricted = bound(&original, &["bank-a"]);

    let out = dispatch(
        &restricted,
        "verify",
        &json!({ "bank": "bank-b", "timestamp": TS }),
    );
    assert_eq!(out["ok"], Value::Bool(false), "{out}");
    assert_eq!(out["code"], "forbidden", "{out}");
    assert!(
        out["indexable_items"].is_null(),
        "a refused verify still reported the bank: {out}"
    );

    // The same request from the unbound state is served and scoped to the bank
    // it named, so the refusal above is the grant rather than a broken argument.
    let open = dispatch(
        &original,
        "verify",
        &json!({ "bank": "bank-b", "timestamp": TS }),
    );
    assert_eq!(open["scope"], "bank:bank-b", "{open}");
    assert_eq!(open["indexable_items"], 2, "{open}");
}

#[test]
fn bound_repair_with_an_explicit_action_list_stays_inside_the_grant() {
    let original = state(false);
    plant_orphan_edge(&original, "bank-a");
    plant_orphan_edge(&original, "bank-b");
    let restricted = bound(&original, &["bank-a"]);

    // No plan and no bank: the omitted scope is the grant, so the action list
    // repairs the granted bank and never the other one.
    let out = dispatch(
        &restricted,
        "repair",
        &json!({
            "actions": ["drop_orphan_assoc_edge"],
            "confirm": true,
            "timestamp": TS
        }),
    );
    assert_eq!(out["ok"], Value::Bool(true), "{out}");
    assert_eq!(out["bank"], "bank-a", "{out}");
    assert_eq!(out["results"][0]["status"], "repaired", "{out}");

    let granted = dispatch(
        &original,
        "verify",
        &json!({ "bank": "bank-a", "timestamp": TS }),
    );
    assert!(!has_finding(&granted, "ORPHAN_ASSOC_EDGE"), "{granted}");
    let foreign = dispatch(
        &original,
        "verify",
        &json!({ "bank": "bank-b", "timestamp": TS }),
    );
    assert!(
        has_finding(&foreign, "ORPHAN_ASSOC_EDGE"),
        "the repair reached the foreign bank: {foreign}"
    );
}

#[test]
fn bound_reindex_with_an_explicit_foreign_bank_changes_no_foreign_index_row() {
    let original = state(false);
    seed_item(&original, "bank-b", "itm-b");
    let before = index_rows(&original, "bank-b");
    assert_eq!(before, (0, 0), "the fixture starts unindexed");
    let restricted = bound(&original, &["bank-a"]);

    let out = dispatch(
        &restricted,
        "reindex",
        &json!({
            "bank": "bank-b",
            "target": "lexical",
            "dry_run": false,
            "confirm": true,
            "timestamp": TS
        }),
    );
    assert_eq!(out["ok"], Value::Bool(false), "{out}");
    assert_eq!(out["code"], "forbidden", "{out}");
    assert_eq!(
        index_rows(&original, "bank-b"),
        before,
        "the refused reindex built the foreign bank's index"
    );
    assert_eq!(count_items(&original, "bank-b"), 1, "{out}");
}

/// Every operator tool reports its own scope; none may be the whole store.
#[test]
fn bound_caller_without_a_bank_never_reaches_the_whole_store() {
    let original = two_bank_store();
    let restricted = bound(&original, &["bank-a"]);

    // Each tool reports the scope it reached: the bank inventory, the `scope`
    // label, or the `bank` the report ran (or refused) in.
    let expected = [
        ("diagnose", "bank-a"),
        ("verify", "bank:bank-a"),
        ("doctor", "bank-a"),
        ("repair", "bank-a"),
        ("reindex", "bank-a"),
    ];
    for (tool, scope) in expected {
        let out = dispatch(&restricted, tool, &json!({ "timestamp": TS }));
        assert!(!out.to_string().contains("bank-b"), "{tool}: {out}");
        let reached = match tool {
            "diagnose" => sorted_report_banks(&out),
            "verify" => vec![out["scope"].as_str().unwrap_or_default().to_owned()],
            _ => vec![out["bank"].as_str().unwrap_or_default().to_owned()],
        };
        assert_eq!(reached, [scope.to_owned()], "{tool} left its grant: {out}");
    }
}

/// Both banks populated, with distinct volumes so a narrowed report is visible.
fn two_bank_store() -> Arc<McpState> {
    let st = state(false);
    seed_item(&st, "bank-a", "itm-a");
    seed_item(&st, "bank-b", "itm-b1");
    seed_item(&st, "bank-b", "itm-b2");
    st
}

/// A report's bank inventory, sorted: catalog order is not part of the contract.
fn sorted_report_banks(report: &Value) -> Vec<String> {
    let mut banks: Vec<String> = report["banks"]
        .as_array()
        .map(|banks| {
            banks
                .iter()
                .filter_map(|bank| bank.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    banks.sort();
    banks
}

#[test]
fn unbound_diagnose_with_a_bank_argument_reports_the_whole_store() {
    let st = two_bank_store();
    let out = dispatch(
        &st,
        "diagnose",
        &json!({ "bank": "bank-a", "timestamp": TS }),
    );
    assert_eq!(out["ok"], Value::Bool(true), "{out}");
    assert_eq!(sorted_report_banks(&out), ["bank-a", "bank-b"], "{out}");
    assert_eq!(out["indexable_items"], 3, "{out}");
    assert_eq!(out["dense_rows"], 0, "{out}");
    assert_eq!(out["lexical_rows"], 0, "{out}");
}

#[test]
fn unbound_diagnose_without_a_bank_reports_the_same_whole_store() {
    let st = two_bank_store();
    let named = dispatch(
        &st,
        "diagnose",
        &json!({ "bank": "bank-a", "timestamp": TS }),
    );
    let omitted = dispatch(&st, "diagnose", &json!({ "timestamp": TS }));
    assert_eq!(named, omitted, "the bank key changed the unbound report");
    assert_eq!(
        sorted_report_banks(&omitted),
        ["bank-a", "bank-b"],
        "{omitted}"
    );
    assert_eq!(omitted["indexable_items"], 3, "{omitted}");
    assert_eq!(omitted["dense_rows"], 0, "{omitted}");
    assert_eq!(omitted["lexical_rows"], 0, "{omitted}");
}

#[test]
fn unbound_reindex_without_bank_and_shared_enabled_sweeps_the_whole_store() {
    let st = state(true);
    seed_item(&st, "bank-a", "itm-a");
    seed_item(&st, "bank-b", "itm-b");

    let out = dispatch(&st, "reindex", &json!({ "timestamp": TS }));
    // A report-only run applies nothing, so its report is not `ok`.
    assert_eq!(out["ok"], Value::Bool(false), "{out}");
    assert_eq!(out["dry_run"], Value::Bool(true), "{out}");
    assert!(
        out["bank"].is_null(),
        "unbound scope is the whole store: {out}"
    );
    assert_eq!(out["indexable_items"], 2, "{out}");
}

/// The `notes` strings of a verify report.
fn report_notes(report: &Value) -> Vec<&str> {
    report["notes"]
        .as_array()
        .map(|notes| notes.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// `verify` publishes `notes` too, so a bound caller must not read another
/// bank's codes or the note that names them, while the unbound operator on the
/// same bank still gets that note.
#[test]
fn bound_verify_does_not_read_another_bank() {
    let original = state(false);
    seed_item(&original, "bank-b", "itm-b");
    plant_orphan_edge(&original, "bank-b");
    let restricted = bound(&original, &["bank-a"]);

    let scoped = dispatch(&restricted, "verify", &json!({ "timestamp": TS }));
    assert_eq!(scoped["scope"], "bank:bank-a", "{scoped}");
    assert_eq!(scoped["ok"], Value::Bool(true), "{scoped}");
    assert!(
        report_notes(&scoped)
            .iter()
            .all(|note| !note.contains("store-wide findings outside")),
        "verify leaked a store-wide note: {scoped}"
    );
    let body = scoped.to_string();
    assert!(
        !body.contains("bank:bank-b"),
        "verify scoped a finding to the foreign bank: {body}"
    );
    // Only the seeded foreign bank can carry these, so a body that names one of
    // them came from outside the caller's grant.
    for code in [
        "ORPHAN_ASSOC_EDGE",
        "DENSE_COVERAGE_GAP",
        "LEXICAL_COVERAGE_GAP",
    ] {
        assert!(
            !body.contains(code),
            "verify response names the foreign bank's {code}: {body}"
        );
    }

    // The same bank from the unbound state does emit the note, so the silence
    // above is the grant rather than a probe that never ran.
    let open = dispatch(
        &original,
        "verify",
        &json!({ "bank": "bank-a", "timestamp": TS }),
    );
    assert_eq!(open["scope"], "bank:bank-a", "{open}");
    assert_eq!(open["ok"], Value::Bool(false), "{open}");
    assert!(
        report_notes(&open)
            .iter()
            .any(|note| note.contains("store-wide findings outside bank `bank-a`")),
        "the unbound operator lost the store-wide note: {open}"
    );
    assert!(
        report_notes(&open)
            .iter()
            .any(|note| note.contains("ORPHAN_ASSOC_EDGE")),
        "the store-wide note names no foreign finding: {open}"
    );
}
