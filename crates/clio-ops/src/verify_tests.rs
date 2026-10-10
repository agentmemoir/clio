// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit tests of the `verify` operator surface:
//! referential-integrity findings, coverage-gap findings, the manifest note,
//! and error-code extraction.
//!
//! ## Owns
//! - `verify` clean/orphan/coverage-gap reports in bank and store scopes
//! - `error_codes` extraction from mixed and empty finding sets
//!
//! ## Does not own
//! - The diagnose snapshot (`diagnose`)
//! - Repair planning or execution (`doctor` / `repair`)
//!
//! ## Boundary
//!
//! Changes to the consistency check set should normally update tests here.
//! Changes to the diagnose snapshot should not require changes here.
//!
//! Keep this module focused on verify behavior.

use crate::finding::{OpsFinding, Severity, codes};
use crate::verify::{VerifyArgs, error_codes, verify};
use clio_store::{AssocEdgeRecord, AssocKind, IntegritySignals, OpsStore, SqliteStore, Store};
use clio_types::{AmError, EpisodicType, EpistemicKind, ItemKind, MemoryItem};
use serde_json::json;
use std::sync::Arc;

const TS: &str = "2026-01-01T00:00:00Z";

fn store() -> SqliteStore {
    SqliteStore::open_in_memory(Arc::new(clio_store::LocalDevKms::new())).expect("in-memory store")
}

fn item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.to_owned(),
        bank_id: bank.to_owned(),
        subject_id: format!("subj-{bank}"),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Task),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.6),
        source_ref: Some("turn_1".to_owned()),
        snapshot: Some(json!({"error_code": "ECONNRESET"})),
        gist: Some("verify fixture".to_owned()),
        valid_from: TS.to_owned(),
        valid_until: None,
        tx_from: TS.to_owned(),
        tx_until: None,
        created_at: TS.to_owned(),
        updated_at: TS.to_owned(),
        context: None,
    }
}

fn orphan_edge(bank: &str) -> AssocEdgeRecord {
    AssocEdgeRecord {
        id: "orphan-1".to_owned(),
        bank_id: bank.to_owned(),
        source_id: "missing-a".to_owned(),
        target_id: "missing-b".to_owned(),
        assoc_kind: AssocKind::Explicit,
        relationship: Some("r".to_owned()),
        weight: 0.5,
        last_updated_at: TS.to_owned(),
    }
}

fn verify_bank(store: &SqliteStore, bank: Option<&str>) -> crate::verify::VerifyReport {
    verify(&VerifyArgs {
        store,
        index: store,
        bank,
        timestamp: TS,
    })
    .expect("verify")
}

struct AllSignalsOps {
    signals: IntegritySignals,
}

impl OpsStore for AllSignalsOps {
    fn ping(&self) -> Result<(), AmError> {
        Ok(())
    }
    fn schema_version(&self) -> Result<Option<String>, AmError> {
        Ok(None)
    }
    fn embedding_space(&self) -> Result<clio_store::EmbeddingSpace, AmError> {
        Ok(clio_store::EmbeddingSpace::default_space())
    }
    fn bank_ids(&self) -> Result<Vec<String>, AmError> {
        Ok(Vec::new())
    }
    fn counts(&self) -> Result<clio_store::StoreCounts, AmError> {
        Ok(clio_store::StoreCounts::default())
    }

    fn integrity_signals(&self, _bank: Option<&str>) -> Result<IntegritySignals, AmError> {
        Ok(self.signals)
    }
    fn drop_orphan_assoc_edges(&self, _bank: &str) -> Result<u64, AmError> {
        Ok(0)
    }
}

#[test]
fn clean_store_is_ok_with_manifest_note() {
    let store = store();
    let report = verify_bank(&store, None);
    assert!(report.ok);
    assert_eq!(report.scope, "store");
    assert_eq!(report.timestamp, TS);
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.code == codes::MANIFEST_UNAVAILABLE)
    );
    assert!(
        report
            .findings
            .iter()
            .all(|f| f.severity != Severity::Error)
    );
    assert_eq!(report.integrity.orphan_assoc_edges, 0);
}

#[test]
fn orphan_edge_is_an_error_finding() {
    let store = store();
    store.put_assoc_edge(&orphan_edge("b")).unwrap();
    let report = verify_bank(&store, Some("b"));
    assert!(!report.ok);
    assert_eq!(report.scope, "bank:b");
    assert_eq!(report.integrity.orphan_assoc_edges, 1);
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.code == codes::ORPHAN_ASSOC_EDGE)
    );
    let failing = error_codes(&report.findings);
    assert_eq!(failing, vec![codes::ORPHAN_ASSOC_EDGE.to_owned()]);
}

#[test]
fn seeded_items_without_indexing_show_coverage_gaps() {
    let store = store();
    store.create_memory_item(&item("i1", "b"), "actor").unwrap();
    store.create_memory_item(&item("i2", "b"), "actor").unwrap();
    let report = verify_bank(&store, Some("b"));
    assert!(!report.ok);
    assert_eq!(report.indexable_items, 2);
    assert_eq!(report.dense_rows, 0);
    assert_eq!(report.lexical_rows, 0);
    let failing = error_codes(&report.findings);
    assert!(failing.contains(&codes::DENSE_COVERAGE_GAP.to_owned()));
    assert!(failing.contains(&codes::LEXICAL_COVERAGE_GAP.to_owned()));
}

#[test]
fn store_wide_scope_reports_every_bank() {
    let store = store();
    store
        .create_memory_item(&item("i1", "b1"), "actor")
        .unwrap();
    store
        .create_memory_item(&item("i2", "b2"), "actor")
        .unwrap();
    let report = verify_bank(&store, None);
    assert_eq!(report.scope, "store");
    assert_eq!(report.indexable_items, 2);
    assert!(!report.ok);
}

#[test]
fn every_integrity_signal_emits_its_finding() {
    let store = store();
    let ops = AllSignalsOps {
        signals: IntegritySignals {
            orphan_assoc_edges: 1,
            orphan_triple_items: 1,
            dangling_snapshot_refs: 1,
            dangling_gist_refs: 1,
            orphan_memtree_leaf_items: 1,
        },
    };
    let report = verify(&VerifyArgs {
        store: &ops,
        index: &store,
        bank: None,
        timestamp: TS,
    })
    .expect("verify");
    assert!(!report.ok);
    for code in [
        codes::ORPHAN_ASSOC_EDGE,
        codes::DANGLING_TRIPLE_REF,
        codes::DANGLING_SNAPSHOT_REF,
        codes::DANGLING_GIST_REF,
        codes::ORPHAN_MEMTREE_LEAF,
    ] {
        assert!(report.findings.iter().any(|f| f.code == code), "{code}");
    }
}

#[test]
fn error_codes_ignores_info_and_warning() {
    let findings = vec![
        OpsFinding::new(codes::MANIFEST_UNAVAILABLE, Severity::Info, "store", "note"),
        OpsFinding::new(
            codes::EMBEDDING_UNREACHABLE,
            Severity::Warning,
            "store",
            "warn",
        ),
    ];
    let none = error_codes(&findings);
    assert!(none.is_empty(), "{none:?}");

    let mixed = vec![
        OpsFinding::new(codes::MANIFEST_UNAVAILABLE, Severity::Info, "store", "note"),
        OpsFinding::new(codes::ORPHAN_ASSOC_EDGE, Severity::Error, "store", "bad"),
    ];
    assert_eq!(
        error_codes(&mixed),
        vec![codes::ORPHAN_ASSOC_EDGE.to_owned()]
    );
}
