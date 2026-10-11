// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit tests of the `diagnose` operator surface:
//! the healthy snapshot, probe-failure findings, schema-version variants, and
//! embedder readiness.
//!
//! ## Owns
//! - `diagnose` report fields and findings across healthy and dead backends
//! - Local `OpsStore` / `IndexStore` / `Embedder` test doubles
//!
//! ## Does not own
//! - Deeper consistency checks (`verify`)
//! - Repair planning or execution (`doctor` / `repair`)
//!
//! ## Boundary
//!
//! Changes to the diagnose probe set should normally update tests here.
//! Changes to storage SQL should not require changes here.
//!
//! Keep this module focused on diagnose behavior.

use crate::diagnose::{DiagnoseArgs, diagnose};
use crate::finding::{Severity, codes};
use clio_index::Embedder;
use clio_store::{
    EMBEDDING_DIMS, IndexCoverage, IndexStore, IntegritySignals, OpsStore, SCHEMA_VERSION,
    SqliteStore, Store,
};
use clio_types::{AmError, EpisodicType, EpistemicKind, ErrorCode, ItemKind, MemoryItem};
use serde_json::json;
use std::sync::Arc;

const TS: &str = "2026-01-01T00:00:00Z";

fn sqlite() -> SqliteStore {
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
        gist: Some("diagnose fixture".to_owned()),
        valid_from: TS.to_owned(),
        valid_until: None,
        tx_from: TS.to_owned(),
        tx_until: None,
        created_at: TS.to_owned(),
        updated_at: TS.to_owned(),
        context: None,
    }
}

fn failed(what: &str) -> AmError {
    let mut text = String::from("\n");
    text.push_str(&what.repeat(60));
    AmError::new(ErrorCode::Internal, text)
}

#[allow(clippy::struct_excessive_bools)]
struct FakeOps {
    ping_ok: bool,
    schema: Result<Option<String>, ()>,
    banks_ok: bool,
    integrity_ok: bool,
    space_ok: bool,
}

impl OpsStore for FakeOps {
    fn ping(&self) -> Result<(), AmError> {
        if self.ping_ok {
            Ok(())
        } else {
            Err(failed("ping failed"))
        }
    }

    fn schema_version(&self) -> Result<Option<String>, AmError> {
        self.schema
            .clone()
            .map_err(|()| failed("schema unreadable"))
    }

    fn embedding_space(&self) -> Result<clio_store::EmbeddingSpace, AmError> {
        if self.space_ok {
            Ok(clio_store::EmbeddingSpace::new("fake/model", 384).expect("space"))
        } else {
            Err(failed("space unreadable"))
        }
    }

    fn bank_ids(&self) -> Result<Vec<String>, AmError> {
        if self.banks_ok {
            Ok(vec!["b1".to_owned()])
        } else {
            Err(failed("banks unreadable"))
        }
    }

    fn bank_exists(&self, _bank_id: &str) -> Result<bool, AmError> {
        Ok(self.banks_ok)
    }

    fn register_bank(&self, _bank_id: &str, _created_at: &str) -> Result<(), AmError> {
        Ok(())
    }

    fn counts(&self) -> Result<clio_store::StoreCounts, AmError> {
        Ok(clio_store::StoreCounts::default())
    }

    fn integrity_signals(&self, _bank: Option<&str>) -> Result<IntegritySignals, AmError> {
        if self.integrity_ok {
            Ok(IntegritySignals::default())
        } else {
            Err(failed("integrity unreadable"))
        }
    }

    fn drop_orphan_assoc_edges(&self, _bank: &str) -> Result<u64, AmError> {
        Ok(0)
    }
}

fn fake_ops(schema: Option<&str>) -> FakeOps {
    FakeOps {
        ping_ok: true,
        schema: Ok(schema.map(str::to_owned)),
        banks_ok: true,
        integrity_ok: true,
        space_ok: true,
    }
}

struct FakeIndex {
    coverage_ok: bool,
}

impl IndexStore for FakeIndex {
    fn index_coverage(&self, bank: Option<&str>) -> Result<IndexCoverage, AmError> {
        if self.coverage_ok {
            Ok(IndexCoverage {
                bank_id: bank.map(str::to_owned),
                indexable_items: 2,
                dense_rows: 1,
                lexical_rows: 1,
                pending_jobs: 0,
                retry_jobs: 0,
                last_error: None,
            })
        } else {
            Err(failed("coverage unreadable"))
        }
    }

    fn put_lexical_doc(&self, _b: &str, _i: &str, _t: &str) -> Result<(), AmError> {
        unimplemented!()
    }
    fn delete_lexical_doc(&self, _b: &str, _i: &str) -> Result<bool, AmError> {
        unimplemented!()
    }
    fn lexical_search(
        &self,
        _b: &str,
        _q: &str,
        _d: &[clio_types::SearchDomain],
        _l: usize,
    ) -> Result<Vec<clio_store::SearchHit>, AmError> {
        unimplemented!()
    }
    fn dense_search(
        &self,
        _b: &str,
        _q: &[f32],
        _d: &[clio_types::SearchDomain],
        _l: usize,
    ) -> Result<Vec<clio_store::KnnHit>, AmError> {
        unimplemented!()
    }
    fn enqueue_index_job(&self, _b: &str, _i: &str) -> Result<bool, AmError> {
        unimplemented!()
    }
    fn pending_index_jobs(
        &self,
        _l: usize,
        _c: Option<&str>,
        _m: u32,
    ) -> Result<Vec<clio_store::IndexJob>, AmError> {
        unimplemented!()
    }
    fn finish_index_job(&self, _b: &str, _i: &str) -> Result<(), AmError> {
        unimplemented!()
    }
    fn fail_index_job(&self, _b: &str, _i: &str, _e: &str) -> Result<(), AmError> {
        unimplemented!()
    }
    fn indexable_items(&self, _b: &str, _l: usize, _o: usize) -> Result<Vec<String>, AmError> {
        unimplemented!()
    }
    fn count_indexable_items(&self, _b: &str) -> Result<u64, AmError> {
        unimplemented!()
    }
    fn indexable_banks(&self) -> Result<Vec<String>, AmError> {
        unimplemented!()
    }
    fn cleanup_orphans(&self, _b: Option<&str>) -> Result<u64, AmError> {
        unimplemented!()
    }
}

struct FakeEmbedder;

impl Embedder for FakeEmbedder {
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AmError> {
        Ok(texts
            .iter()
            .map(|_| vec![0.0_f32; EMBEDDING_DIMS])
            .collect())
    }

    fn health(&self) -> Result<(), AmError> {
        Ok(())
    }

    fn model_id(&self) -> String {
        clio_store::DEFAULT_EMBEDDING_MODEL.to_owned()
    }

    fn dims(&self) -> usize {
        EMBEDDING_DIMS
    }
}

struct FailingEmbedder;

impl Embedder for FailingEmbedder {
    fn embed_batch(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, AmError> {
        Err(AmError::new(ErrorCode::Internal, "sidecar down"))
    }

    fn health(&self) -> Result<(), AmError> {
        Err(AmError::new(ErrorCode::Internal, "sidecar down"))
    }

    fn model_id(&self) -> String {
        "fake/failing".to_owned()
    }

    fn dims(&self) -> usize {
        EMBEDDING_DIMS
    }
}

#[test]
fn healthy_store_snapshot_has_expected_fields() {
    let store = sqlite();
    store
        .create_memory_item(&item("i1", "b1"), "actor")
        .unwrap();
    let report = diagnose(&DiagnoseArgs {
        store: &store,
        index: &store,
        embedder: None,
        timestamp: TS,
    });
    assert!(report.ok);
    assert_eq!(report.timestamp, TS);
    assert!(report.backend_reachable);
    assert_eq!(report.schema_version.as_deref(), Some(SCHEMA_VERSION));
    assert_eq!(report.expected_schema_version, SCHEMA_VERSION);
    assert_eq!(report.banks, vec!["b1".to_owned()]);
    assert!(report.vector_index_present);
    // The active embedding space and its index mode are reported.
    assert_eq!(
        report.embedding_model.as_deref(),
        Some("BAAI/bge-small-en-v1.5")
    );
    assert_eq!(report.embedding_dims, Some(384));
    assert_eq!(report.index_mode.as_deref(), Some("ann_vector"));
    assert_eq!(report.indexable_items, 1);
    assert_eq!(report.embedding_ready, None);
    assert!(report.findings.is_empty(), "{:?}", report.findings);
}

#[test]
fn exact_scan_space_is_reported() {
    let mut store = sqlite();
    let space = clio_store::EmbeddingSpace::new("wide/model", 4096).unwrap();
    // A fresh store has no derived rows, so no destructive-switch opt-in.
    store.apply_configured_space(&space, false).unwrap();
    let report = diagnose(&DiagnoseArgs {
        store: &store,
        index: &store,
        embedder: None,
        timestamp: TS,
    });
    assert_eq!(report.embedding_model.as_deref(), Some("wide/model"));
    assert_eq!(report.embedding_dims, Some(4096));
    assert_eq!(report.index_mode.as_deref(), Some("exact_scan"));
}

#[test]
fn healthy_embedder_reports_ready() {
    let store = sqlite();
    let embedder = FakeEmbedder;
    let report = diagnose(&DiagnoseArgs {
        store: &store,
        index: &store,
        embedder: Some(&embedder),
        timestamp: TS,
    });
    assert!(report.ok);
    assert_eq!(report.embedding_ready, Some(true));
}

#[test]
fn failing_embedder_is_a_warning_not_a_failure() {
    let store = sqlite();
    let embedder = FailingEmbedder;
    let report = diagnose(&DiagnoseArgs {
        store: &store,
        index: &store,
        embedder: Some(&embedder),
        timestamp: TS,
    });
    assert!(report.ok);
    assert_eq!(report.embedding_ready, Some(false));
    assert!(
        report
            .findings
            .iter()
            .any(|f| { f.code == codes::EMBEDDING_UNREACHABLE && f.severity == Severity::Warning })
    );
}

#[test]
fn fake_backend_matching_schema_is_ok() {
    let ops = fake_ops(Some(SCHEMA_VERSION));
    let index = FakeIndex { coverage_ok: true };
    let report = diagnose(&DiagnoseArgs {
        store: &ops,
        index: &index,
        embedder: None,
        timestamp: TS,
    });
    assert!(report.ok);
    assert_eq!(report.indexable_items, 2);
    assert_eq!(report.dense_rows, 1);
}

#[test]
fn missing_schema_row_is_an_error() {
    let ops = fake_ops(None);
    let index = FakeIndex { coverage_ok: true };
    let report = diagnose(&DiagnoseArgs {
        store: &ops,
        index: &index,
        embedder: None,
        timestamp: TS,
    });
    assert!(!report.ok);
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.code == codes::SCHEMA_VERSION_MISSING)
    );
}

#[test]
fn schema_mismatch_is_an_error() {
    let ops = fake_ops(Some("0"));
    let index = FakeIndex { coverage_ok: true };
    let report = diagnose(&DiagnoseArgs {
        store: &ops,
        index: &index,
        embedder: None,
        timestamp: TS,
    });
    assert!(!report.ok);
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.code == codes::SCHEMA_VERSION_MISMATCH)
    );
}

#[test]
fn dead_backend_reports_every_probe_failure() {
    let ops = FakeOps {
        ping_ok: false,
        schema: Err(()),
        banks_ok: false,
        integrity_ok: false,
        space_ok: false,
    };
    let index = FakeIndex { coverage_ok: true };
    let report = diagnose(&DiagnoseArgs {
        store: &ops,
        index: &index,
        embedder: None,
        timestamp: TS,
    });
    assert!(!report.ok);
    assert!(!report.backend_reachable);
    assert!(report.schema_version.is_none());
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.code == codes::BACKEND_UNREACHABLE)
    );
    let unreadable = report
        .findings
        .iter()
        .filter(|f| f.code == codes::SYSTEM_TABLES_UNREADABLE)
        .count();
    assert_eq!(unreadable, 4, "schema, banks, integrity, embedding space");
    assert!(report.embedding_model.is_none());
    assert!(report.embedding_dims.is_none());
    assert!(report.index_mode.is_none());
}

#[test]
fn unreadable_index_state_is_a_finding() {
    let ops = fake_ops(Some(SCHEMA_VERSION));
    let index = FakeIndex { coverage_ok: false };
    let report = diagnose(&DiagnoseArgs {
        store: &ops,
        index: &index,
        embedder: None,
        timestamp: TS,
    });
    assert!(!report.ok);
    assert!(!report.vector_index_present);
    assert_eq!(report.indexable_items, 0);
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.code == codes::VECTOR_INDEX_MISSING)
    );
}
