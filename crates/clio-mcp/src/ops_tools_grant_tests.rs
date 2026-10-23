// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that a bound MCP caller's operator tools stay inside the
//! credential's bank grant: a registered plan, an omitted bank, or a whole-store
//! sweep must never reach a bank the grant excludes.
//!
//! ## Owns
//! - The refusal a registered foreign plan produces, and the plan, edge, and
//!   index state it leaves behind
//! - The `doctor` plan and `notes` a bound caller reads, and the store-wide note
//!   an unbound operator still gets
//! - The bounded `reindex` sweep in both shared-enable states
//! - The fixtures every bound-scope test in this module family is built from
//!
//! ## Does not own
//! - The per-tool scope assertions (`ops_tools_grant_scope_tests`)
//! - Operator semantics themselves (`clio-ops`)
//! - Argument decoding and the `{ok}` envelope (`ops_tools_tests`)
//!
//! ## Boundary
//!
//! Tests here break when an operator tool's final scope or grant check changes;
//! they never change it.
//!
//! Keep this module focused on the bank grant of a bound operator caller.

use crate::principal::Principal;
use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_store::ItemInspectFilter;
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) const TS: &str = "2026-01-01T00:00:00Z";

/// An id no test registers: plan ids are allocated from one, per process.
const ABSENT_PLAN: &str = "plan-404";

/// One unbound in-memory state: `bank-a` is the server default.
pub(super) fn state(shared: bool) -> Arc<McpState> {
    McpState::open_with_effective(
        &McpOpenOptions {
            backend: "sqlite".into(),
            database_url: "sqlite::memory:".into(),
            default_bank: "bank-a".into(),
            default_actor: clio_types::Actor::Agent,
            shared_bank_enabled: shared,
            kms_path: None,
        },
        &json!({}),
    )
    .expect("mcp state")
}

/// A restricted view of the same store, sharing every field but the principal.
pub(super) fn bound(state: &Arc<McpState>, banks: &[&str]) -> Arc<McpState> {
    Arc::new(state.with_principal(Principal::new(
        "restricted".into(),
        clio_types::Actor::Harness,
        banks.iter().map(|bank| (*bank).to_owned()).collect(),
    )))
}

pub(super) fn seed_item(st: &McpState, bank: &str, id: &str) {
    let item = clio_types::MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: format!("subj-{id}"),
        kind: clio_types::ItemKind::Semantic,
        category: Some(clio_types::SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.2),
        source_ref: None,
        snapshot: None,
        gist: Some("mcp ops grant fixture item".into()),
        valid_from: TS.into(),
        valid_until: None,
        tx_from: TS.into(),
        tx_until: None,
        created_at: TS.into(),
        updated_at: TS.into(),
        context: None,
    };
    st.store.create_memory_item(&item, "actor").expect("seed");
}

pub(super) fn plant_orphan_edge(st: &McpState, bank: &str) {
    st.store
        .put_assoc_edge(&clio_store::AssocEdgeRecord {
            id: format!("ghost-edge-{bank}"),
            bank_id: bank.into(),
            source_id: "ghost-a".into(),
            target_id: "ghost-b".into(),
            assoc_kind: clio_store::AssocKind::Explicit,
            relationship: Some("r".into()),
            weight: 0.5,
            last_updated_at: TS.into(),
        })
        .expect("plant orphan edge");
}

pub(super) fn has_finding(value: &Value, code: &str) -> bool {
    value["findings"]
        .as_array()
        .is_some_and(|findings| findings.iter().any(|f| f["code"] == code))
}

/// The `notes` strings of a doctor plan.
fn plan_notes(plan: &Value) -> Vec<&str> {
    plan["notes"]
        .as_array()
        .map(|notes| notes.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// Error codes only the seeded foreign bank can carry, so a note or a body that
/// names one of them came from outside the caller's grant.
const FOREIGN_ONLY_CODES: [&str; 3] = [
    "ORPHAN_ASSOC_EDGE",
    "DENSE_COVERAGE_GAP",
    "LEXICAL_COVERAGE_GAP",
];

/// Register a live plan for `bank` through the unbound state, as another
/// caller would, and return its id.
fn registered_plan(st: &McpState, bank: &str) -> String {
    let out = dispatch(
        st,
        "doctor",
        &json!({ "bank": bank, "dry_run": false, "timestamp": TS }),
    );
    assert!(
        out["actions"]
            .as_array()
            .is_some_and(|actions| actions.iter().any(|a| a["id"] == "drop_orphan_assoc_edge")),
        "{out}"
    );
    out["plan_id"].as_str().expect("plan id").to_owned()
}

pub(super) fn index_rows(st: &McpState, bank: &str) -> (u64, u64) {
    let coverage = st.index.index_coverage(Some(bank)).expect("coverage");
    (coverage.dense_rows, coverage.lexical_rows)
}

pub(super) fn count_items(st: &McpState, bank: &str) -> usize {
    st.store
        .count_items(&ItemInspectFilter::new(bank))
        .expect("count")
}

/// A refusal body with the caller's own plan id normalized away.
///
/// The one text a foreign id and an unknown id may not share is the id itself,
/// which the message echoes from the request; everything else a caller can see
/// must be identical.
fn body_without_plan_id(value: &Value, plan_id: &str) -> String {
    value.to_string().replace(plan_id, "<plan-id>")
}

#[test]
fn bound_repair_with_a_foreign_explicit_bank_is_forbidden() {
    let original = state(false);
    let restricted = bound(&original, &["bank-a"]);
    let out = dispatch(
        &restricted,
        "repair",
        &json!({
            "bank": "bank-b",
            "actions": ["drop_orphan_assoc_edge"],
            "confirm": true,
            "timestamp": TS
        }),
    );
    assert_eq!(out["ok"], Value::Bool(false), "{out}");
    assert_eq!(out["code"], "forbidden", "{out}");
}

#[test]
fn bound_repair_cannot_reach_the_bank_of_a_foreign_plan() {
    let original = state(false);
    seed_item(&original, "bank-b", "itm-b");
    plant_orphan_edge(&original, "bank-b");
    let plan_id = registered_plan(&original, "bank-b");
    let restricted = bound(&original, &["bank-a"]);

    let foreign = dispatch(
        &restricted,
        "repair",
        &json!({ "plan_id": plan_id, "confirm": true, "timestamp": TS }),
    );
    // A plan the grant does not cover answers exactly like an id that was never
    // registered: ids are sequential, so any difference would say that another
    // bank exists.
    let absent = dispatch(
        &restricted,
        "repair",
        &json!({ "plan_id": ABSENT_PLAN, "confirm": true, "timestamp": TS }),
    );
    assert_eq!(foreign["ok"], Value::Bool(false), "{foreign}");
    assert_eq!(
        foreign["findings"][0]["code"], "PLAN_NOT_FOUND",
        "{foreign}"
    );
    assert_eq!(
        foreign["findings"][0]["scope"], absent["findings"][0]["scope"],
        "the refusal scoped a foreign plan differently: {foreign} / {absent}"
    );
    assert_eq!(foreign["bank"], absent["bank"], "{foreign} / {absent}");
    assert_eq!(
        foreign["plan_id"], absent["plan_id"],
        "{foreign} / {absent}"
    );
    assert_eq!(foreign["results"], json!([]), "{foreign}");
    assert_eq!(
        body_without_plan_id(&foreign, &plan_id),
        body_without_plan_id(&absent, ABSENT_PLAN),
        "a foreign plan id answered differently from an unregistered one"
    );
    assert!(
        !foreign.to_string().contains("bank-b"),
        "the refusal names the foreign bank: {foreign}"
    );

    // Nothing was hidden in the registry itself: the other caller's plan is
    // still there, and the refusal above ran none of its actions.
    assert!(
        original.ops_plans.get(&plan_id).is_some(),
        "the foreign plan was removed from the registry"
    );
    let check = dispatch(
        &original,
        "verify",
        &json!({ "bank": "bank-b", "timestamp": TS }),
    );
    assert!(has_finding(&check, "ORPHAN_ASSOC_EDGE"), "{check}");
}

#[test]
fn bound_repair_without_bank_uses_a_granted_plans_bank() {
    let original = state(false);
    plant_orphan_edge(&original, "bank-a");
    let plan_id = registered_plan(&original, "bank-a");
    let restricted = bound(&original, &["bank-a"]);

    let out = dispatch(
        &restricted,
        "repair",
        &json!({ "plan_id": plan_id, "confirm": true, "timestamp": TS }),
    );
    assert_eq!(out["ok"], Value::Bool(true), "{out}");
    assert_eq!(out["bank"], "bank-a", "{out}");

    let check = dispatch(
        &original,
        "verify",
        &json!({ "bank": "bank-a", "timestamp": TS }),
    );
    assert!(!has_finding(&check, "ORPHAN_ASSOC_EDGE"), "{check}");
}

/// A bound `reindex` without `bank`, in one shared-enable state.
fn assert_bound_reindex_scopes_to_the_grant(shared: bool) {
    let original = state(shared);
    seed_item(&original, "bank-a", "itm-a");
    seed_item(&original, "bank-b", "itm-b");
    let foreign_before = index_rows(&original, "bank-b");
    let restricted = bound(&original, &["bank-a"]);

    let out = dispatch(
        &restricted,
        "reindex",
        &json!({ "target": "lexical", "dry_run": false, "confirm": true, "timestamp": TS }),
    );
    assert_eq!(out["ok"], Value::Bool(true), "shared={shared}: {out}");
    assert_eq!(out["bank"], "bank-a", "shared={shared}: {out}");
    assert_eq!(out["indexable_items"], 1, "shared={shared}: {out}");
    assert_eq!(index_rows(&original, "bank-a").1, 1);
    assert_eq!(index_rows(&original, "bank-b"), foreign_before);
    assert_eq!(count_items(&original, "bank-b"), 1);
}

#[test]
fn bound_reindex_without_bank_scopes_to_the_grant_with_shared_disabled() {
    assert_bound_reindex_scopes_to_the_grant(false);
}

#[test]
fn bound_reindex_without_bank_still_scopes_to_the_grant_with_shared_enabled() {
    assert_bound_reindex_scopes_to_the_grant(true);
}

#[test]
fn bound_verify_and_doctor_without_bank_never_scope_to_a_foreign_bank() {
    let original = state(false);
    seed_item(&original, "bank-b", "itm-b");
    plant_orphan_edge(&original, "bank-b");
    let restricted = bound(&original, &["bank-a"]);

    let verify = dispatch(&restricted, "verify", &json!({ "timestamp": TS }));
    assert_eq!(verify["ok"], Value::Bool(true), "{verify}");
    assert_eq!(verify["scope"], "bank:bank-a", "{verify}");
    assert!(!has_finding(&verify, "ORPHAN_ASSOC_EDGE"), "{verify}");

    let doctor = dispatch(&restricted, "doctor", &json!({ "timestamp": TS }));
    assert_eq!(doctor["bank"], "bank-a", "{doctor}");
    let actions: Vec<&str> = doctor["actions"]
        .as_array()
        .map(|actions| actions.iter().filter_map(|a| a["id"].as_str()).collect())
        .unwrap_or_default();
    assert!(
        !actions.contains(&"drop_orphan_assoc_edge"),
        "doctor planned the foreign bank's repair: {doctor}"
    );

    // A dry run stores nothing, so the plan a bound caller then composes live is
    // the one to read: its `bank` is its own, never the whole store (`null`).
    let live = dispatch(
        &restricted,
        "doctor",
        &json!({ "timestamp": TS, "dry_run": false }),
    );
    assert_eq!(live["bank"], "bank-a", "{live}");
    let live_id = live["plan_id"].as_str().expect("a plan id");
    let stored = restricted
        .ops_plans
        .get(live_id)
        .expect("the plan it composed");
    assert_eq!(
        stored.bank.as_deref(),
        Some("bank-a"),
        "the bound plan spans the whole store"
    );

    // `notes` is a second field with its own disclosure rule: a bound caller
    // must not read that another bank exists or which codes it carries.
    let notes = plan_notes(&doctor);
    assert!(
        !notes
            .iter()
            .any(|note| note.contains("store-wide findings outside")),
        "doctor leaked a store-wide note: {doctor}"
    );
    let body = doctor.to_string();
    assert!(
        !body.contains("bank:bank-b"),
        "doctor scoped a finding to the foreign bank: {body}"
    );
    for code in FOREIGN_ONLY_CODES {
        assert!(
            !notes.iter().any(|note| note.contains(code)),
            "doctor note names the foreign bank's {code}: {doctor}"
        );
        assert!(
            !body.contains(code),
            "doctor response names the foreign bank's {code}: {body}"
        );
    }

    // The same scope from the unbound state does emit the note, so the silence
    // above is the grant rather than a probe that never ran.
    let open = dispatch(
        &original,
        "doctor",
        &json!({ "bank": "bank-a", "timestamp": TS }),
    );
    assert_eq!(open["bank"], "bank-a", "{open}");
    let open_notes = plan_notes(&open);
    assert!(
        open_notes
            .iter()
            .any(|note| note.contains("store-wide findings outside bank `bank-a`")),
        "the unbound operator lost the store-wide note: {open}"
    );
    assert!(
        open_notes
            .iter()
            .any(|note| note.contains("ORPHAN_ASSOC_EDGE")),
        "the store-wide note names no foreign finding: {open}"
    );
}
