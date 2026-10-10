// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for `hub_distill` acceptance tests: over-degree
//! detection, gated distillation, admission-reject edge retention, and the
//! `consolidate` integration surface.
//!
//! ## Owns
//!
//! Test-only. Never compiled into production builds.

use crate::hub_distill::{HUB_DISTILL_JOB, HUB_DISTILL_MARKER, HubDistillEngine};
use crate::maintenance::LeafIndex;
use crate::memtree::MemForest;
use crate::memtree_maint::MemTreeMaint;
use crate::memtree_tools::consolidate_tool;
use clio_config::ranking::RankingEnv;
use clio_config::retention::{ResolvedRetention, RetentionProfile, Verbosity};
use clio_store::{AssocKind, AssocPolicy, LocalDevKms, SqliteStore, Store};
use clio_types::{
    Actor, Bank, EpistemicKind, ErrorCode, ItemKind, MemoryItem, OpContext, SemanticCategory,
    ToolResult,
};
use std::sync::{Arc, Mutex};

pub(crate) fn store() -> Arc<SqliteStore> {
    let kms: Arc<dyn clio_store::DekProvider> = Arc::new(LocalDevKms::new());
    Arc::new(SqliteStore::open_in_memory(kms).expect("sqlite"))
}

pub(crate) fn semantic(id: &str, bank: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.6),
        source_ref: Some("turn".into()),
        snapshot: Some(serde_json::json!({"k": "v"})),
        gist: Some(gist.into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

pub(crate) fn ctx(bank: &str) -> OpContext {
    OpContext::new(Bank::new(bank), Actor::parse("agent").expect("actor"))
}

fn env_with_theta(theta: f64) -> RankingEnv {
    let mut env = RankingEnv::system_defaults();
    env.theta_admit = theta;
    env
}

pub(crate) fn policy(limit: u32) -> AssocPolicy {
    AssocPolicy {
        hub_degree_limit: limit,
        ..AssocPolicy::default()
    }
}

/// A cluster's items stamped in the pinned wave clock's own millisecond, like the
/// coactivation edges below.
///
/// The stamp is the point: `semantic()` writes `updated_at` in the MCP path's ISO
/// form at a 2026 date, while the engines here are pinned at millis 0, so an
/// item left on that stamp looks like it changed long after any reconciliation
/// built from it. That was harmless while a moved stamp was read-time only, and
/// it is not now that a wave records the lapse on the record: every seeded unit
/// would report `source_changed` before any test touched a source. Pinning the
/// stamp to the wave clock makes a unit current for the reason a unit is current,
/// and a test that wants a moved stamp moves one explicitly.
fn stamped_at_wave_clock(mut item: clio_types::MemoryItem) -> clio_types::MemoryItem {
    item.updated_at = WAVE_CLOCK_STAMP.to_owned();
    item
}

/// The millisecond every engine in these tests is pinned at, and the stamp every
/// seeded item carries. Equal, so no seeded unit is lapsed by its own fixture.
pub(crate) const WAVE_CLOCK_STAMP: &str = "0";

pub(crate) fn seed_hub(
    store: &Arc<SqliteStore>,
    bank: &str,
    hub: &str,
    neighbors: &[&str],
    eta: f64,
) {
    store
        .create_memory_item(
            &stamped_at_wave_clock(semantic(hub, bank, "hub core")),
            "agent",
        )
        .unwrap();
    for n in neighbors {
        store
            .create_memory_item(
                &stamped_at_wave_clock(semantic(n, bank, &format!("neighbor {n}"))),
                "agent",
            )
            .unwrap();
        store
            .reinforce_assoc_edge(bank, hub, n, 0, eta)
            .expect("reinforce");
    }
}

pub(crate) fn engine(store: &Arc<SqliteStore>, theta: f64, limit: u32) -> HubDistillEngine {
    engine_at(store, theta, limit, 0)
}

/// The same engine with its clock pinned, so decay is a function of `now_millis`
/// and not of when the suite runs. `seed_hub` stamps its edges at millis 0.
pub(crate) fn engine_at(
    store: &Arc<SqliteStore>,
    theta: f64,
    limit: u32,
    now_millis: i64,
) -> HubDistillEngine {
    HubDistillEngine::with_clock(
        Arc::clone(store) as Arc<dyn Store>,
        env_with_theta(theta),
        policy(limit),
        now_millis,
    )
}

#[test]
fn hub_distill_job_kind_is_distinct() {
    assert_eq!(HUB_DISTILL_JOB, "hub_distill");
    assert_ne!(HUB_DISTILL_JOB, "memtree_refresh");
}

#[test]
fn t09_over_degree_hub_distills_gated_item_and_prunes() {
    let store = store();
    seed_hub(&store, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    let engine = engine(&store, 0.0, 2);
    let eligible = engine.eligible_hubs("bank").unwrap();
    assert_eq!(eligible, vec!["hub".to_owned()]);
    let report = engine
        .consolidate_hubs("bank", &ctx("bank"), &ResolvedRetention::default())
        .unwrap();
    assert_eq!(report.distilled.len(), 1);
    assert!(
        report.rejected.is_empty(),
        "rejected: {:?}",
        report.rejected
    );
    assert!(report.summary().starts_with("hub_distill:"));
    let item_id = "hub-distill:bank:hub".to_owned();
    let item = store
        .get_memory_item("bank", &item_id)
        .unwrap()
        .expect("item");
    assert_eq!(item.kind, ItemKind::Semantic);
    assert_eq!(item.category, Some(SemanticCategory::Schema));
    assert_eq!(item.source_ref.as_deref(), Some(HUB_DISTILL_MARKER));
    assert_ne!(
        item.source_ref.as_deref(),
        Some(HUB_DISTILL_JOB),
        "the recall marker is not the audit job kind"
    );
    // Cluster coactivation edges pruned; explicit cross-refs remain.
    let remaining = store.list_assoc_edges("bank", Some("hub")).unwrap();
    assert!(
        remaining
            .iter()
            .all(|e| e.assoc_kind == AssocKind::Explicit),
        "only cross-ref explicit edges should remain"
    );
    assert!(
        remaining
            .iter()
            .any(|e| e.relationship.as_deref() == Some(HUB_DISTILL_JOB))
    );
}

#[test]
fn t10_admission_reject_keeps_edges_and_no_item() {
    let store = store();
    seed_hub(&store, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    let engine = engine(&store, 1.0, 2);
    let report = engine
        .consolidate_hubs("bank", &ctx("bank"), &ResolvedRetention::default())
        .unwrap();
    assert_eq!(report.rejected, vec!["hub".to_owned()]);
    assert!(report.distilled.is_empty(), "{:?}", report.distilled);
    assert!(
        store
            .get_memory_item("bank", "hub-distill:bank:hub")
            .unwrap()
            .is_none(),
        "rejected distill must not fake a write"
    );
    let coact = store.list_assoc_edges("bank", Some("hub")).unwrap();
    assert_eq!(coact.len(), 3, "cluster edges retained after reject");
    for e in coact {
        assert_eq!(e.assoc_kind, AssocKind::Coactivation);
    }
}

#[test]
fn already_distilled_is_idempotent_when_prune_disabled() {
    let store = store();
    seed_hub(&store, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    let mut engine = engine(&store, 0.0, 2);
    engine.set_prune_after_distill(false);
    let first = engine
        .consolidate_hubs("bank", &ctx("bank"), &ResolvedRetention::default())
        .unwrap();
    assert_eq!(first.distilled.len(), 1);
    let second = engine
        .consolidate_hubs("bank", &ctx("bank"), &ResolvedRetention::default())
        .unwrap();
    // The known unit is reconciled: an unchanged cluster writes nothing.
    assert_eq!(second.reconcile_unchanged, vec!["hub".to_owned()]);
    assert!(second.distilled.is_empty(), "{:?}", second.distilled);
    assert!(
        store
            .get_memory_item("bank", "hub-distill:bank:hub")
            .unwrap()
            .is_some_and(|item| item.tx_until.is_none())
    );
}

#[test]
fn explicit_edges_are_not_counted_and_missing_hub_errors() {
    let store = store();
    store
        .create_memory_item(&semantic("hub", "bank", "hub"), "agent")
        .unwrap();
    for n in ["n1", "n2", "n3"] {
        store
            .create_memory_item(&semantic(n, "bank", n), "agent")
            .unwrap();
        store
            .put_assoc_edge(&clio_store::AssocEdgeRecord {
                id: clio_store::explicit_id("bank", "hub", n, "depends_on"),
                bank_id: "bank".into(),
                source_id: "hub".into(),
                target_id: n.into(),
                assoc_kind: AssocKind::Explicit,
                relationship: Some("depends_on".into()),
                weight: 1.0,
                last_updated_at: "0".into(),
            })
            .unwrap();
    }
    let engine = engine(&store, 0.0, 2);
    let eligible = engine.eligible_hubs("bank").unwrap();
    assert!(eligible.is_empty(), "{eligible:?}");

    // A hub with no edges yields no_neighbors, not a forged item.
    store
        .create_memory_item(&semantic("lonely", "bank", "lonely"), "agent")
        .unwrap();
    let outcome = engine
        .distill_hub("bank", "lonely", &ctx("bank"), ResolvedRetention::default())
        .unwrap();
    assert_eq!(outcome.status, "no_neighbors");
    let missing = engine
        .distill_hub("bank", "ghost", &ctx("bank"), ResolvedRetention::default())
        .expect_err("missing hub");
    assert_eq!(missing.code, ErrorCode::ItemNotFound);
}

#[test]
fn hub_engine_edge_paths_and_report_formatting() {
    let store = store();
    let mut engine = engine(&store, 0.0, 2);
    let sink: Arc<dyn clio_admission::EventSink + Send + Sync> =
        Arc::new(clio_admission::VecSink::default());
    engine.set_sink(sink);
    assert!(format!("{engine:?}").contains("HubDistillEngine"));
    assert_eq!(
        engine.eligible_hubs("").unwrap_err().code,
        ErrorCode::MissingBank
    );
    assert_eq!(
        engine
            .consolidate_hubs("", &ctx("bank"), &ResolvedRetention::default())
            .unwrap_err()
            .code,
        ErrorCode::MissingBank
    );
    assert_eq!(
        engine
            .distill_hub("", "h", &ctx("bank"), ResolvedRetention::default())
            .unwrap_err()
            .code,
        ErrorCode::MissingBank
    );
    let empty = engine
        .consolidate_hubs("bank", &ctx("bank"), &ResolvedRetention::default())
        .unwrap();
    assert!(empty.eligible.is_empty(), "eligible: {:?}", empty.eligible);
    assert!(empty.summary().contains("eligible=0"));
}

#[test]
fn distill_honors_the_bank_retention_profile() {
    // The builtin (balanced) profile admits the distilled item and records its score.
    let relaxed_store = store();
    seed_hub(&relaxed_store, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    let relaxed = engine(&relaxed_store, 0.0, 2);
    let ok = relaxed
        .distill_hub("bank", "hub", &ctx("bank"), ResolvedRetention::default())
        .unwrap();
    assert_eq!(ok.status, "distilled");
    let score = ok.admission_score.expect("balanced distill score");

    // Same fixture and ranking env, but the bank resolves a selective profile:
    // its effective semantic threshold (theta + selective delta) exceeds the
    // balanced score, so the same distilled item is rejected and not written.
    let strict = store();
    seed_hub(&strict, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    let strict_engine = engine(&strict, score, 2);
    let profile = RetentionProfile {
        verbosity: Verbosity::Selective,
        ..RetentionProfile::default()
    };
    let rejected = strict_engine
        .distill_hub("bank", "hub", &ctx("bank"), profile.resolve())
        .unwrap();
    assert_eq!(rejected.status, "rejected");
    assert!(rejected.admission_score.is_some());
    assert!(
        strict
            .get_memory_item("bank", "hub-distill:bank:hub")
            .unwrap()
            .is_none(),
        "rejected distill must not write"
    );
}

#[test]
fn consolidate_tool_runs_hub_distillation() {
    let store = store();
    seed_hub(&store, "bank", "hub", &["n1", "n2", "n3"], 0.15);
    let engine = Arc::new(engine(&store, 0.0, 2));
    let forest = Arc::new(Mutex::new(MemForest::with_fanout(2)));
    let leaves = Arc::new(LeafIndex::new());
    let maint = MemTreeMaint::new(Arc::clone(&forest), leaves);
    let ToolResult::Ok { data, .. } = consolidate_tool(
        &maint,
        Some(&engine),
        Some(&ctx("bank")),
        "bank",
        ResolvedRetention::default(),
        false,
        true,
    ) else {
        panic!("consolidate should succeed");
    };
    assert_eq!(data.status, "completed");
    assert!(data.hubs.contains("distilled=1"), "hubs: {}", data.hubs);
    assert!(
        store
            .get_memory_item("bank", "hub-distill:bank:hub")
            .unwrap()
            .is_some()
    );
    // Missing bank still fails closed.
    assert!(matches!(
        consolidate_tool(
            &maint,
            Some(&engine),
            Some(&ctx("bank")),
            "",
            ResolvedRetention::default(),
            false,
            true
        ),
        ToolResult::Err {
            code: ErrorCode::MissingBank,
            ..
        }
    ));
}
