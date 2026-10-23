// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit tests of the bank-scoped `diagnose`
//! surface: which banks a scoped snapshot lists, which coverage counts it
//! reports, and what it must not disclose about a foreign bank.
//!
//! ## Owns
//! - `diagnose_scoped` bank inventory for an existing, an unknown, and a
//!   whole-store scope
//! - Scoped coverage counts and the disclosure boundary between banks
//!
//! ## Does not own
//! - The unscoped snapshot's probe-failure findings (`diagnose`)
//! - Deeper consistency checks (`verify`)
//!
//! ## Boundary
//!
//! Changes to how a scope selects banks or counts should normally update tests
//! here. Changes to storage SQL should not require changes here.
//!
//! Keep this module focused on scoped diagnose behavior.

use crate::diagnose::{DiagnoseArgs, diagnose_scoped};
use crate::diagnose_fixtures::{TS, failed, item, sqlite};
use crate::finding::{Severity, codes};
use clio_store::{
    DEFAULT_EMBEDDING_MODEL, EMBEDDING_DIMS, EmbeddingSpace, IntegritySignals, OpsStore,
    SCHEMA_VERSION, Store,
};
use clio_types::AmError;

/// Seed one store with two banks: one item in `bank-a`, two in `bank-b`.
fn two_banks() -> clio_store::SqliteStore {
    let store = sqlite();
    store
        .create_memory_item(&item("i1", "bank-a"), "actor")
        .unwrap();
    store
        .create_memory_item(&item("i2", "bank-b"), "actor")
        .unwrap();
    store
        .create_memory_item(&item("i3", "bank-b"), "actor")
        .unwrap();
    store
}

/// The report body as JSON, so a disclosure check covers every field.
fn body(report: &crate::diagnose::DiagnoseReport) -> String {
    serde_json::to_string(report).expect("report json")
}

/// A scope naming an existing bank lists that bank alone, with that bank's own
/// coverage count, and says nothing about the other bank.
#[test]
fn scoped_report_lists_only_the_named_bank() {
    let store = two_banks();
    let scoped = diagnose_scoped(
        &DiagnoseArgs {
            store: &store,
            index: &store,
            embedder: None,
            timestamp: TS,
        },
        Some("bank-a"),
    );
    assert!(scoped.ok, "{:?}", scoped.findings);
    assert_eq!(scoped.banks, vec!["bank-a".to_owned()]);
    // The other bank holds two items; this report counts one of each.
    assert_eq!(scoped.indexable_items, 1);
    assert_eq!(scoped.pending_jobs, 1);
    assert_eq!(scoped.retry_jobs, 0);
    assert!(scoped.findings.iter().all(|f| f.scope != "bank:bank-b"));
    assert!(!body(&scoped).contains("bank-b"), "{}", body(&scoped));
}

/// A scope naming a bank that does not exist lists no bank at all, stays `ok`,
/// and discloses neither the existing bank's name nor its volume.
#[test]
fn unknown_bank_scope_lists_no_bank() {
    let store = two_banks();
    let report = diagnose_scoped(
        &DiagnoseArgs {
            store: &store,
            index: &store,
            embedder: None,
            timestamp: TS,
        },
        Some("ghost-bank"),
    );
    assert!(report.ok, "{:?}", report.findings);
    assert!(report.backend_reachable);
    assert!(report.banks.is_empty(), "{:?}", report.banks);
    // Any finding this scope produces is labeled for the scope, never `store`.
    assert!(
        report.findings.iter().all(|f| f.scope == "bank:ghost-bank"),
        "{:?}",
        report.findings
    );
    // The unseen bank's single item is not counted.
    assert_eq!(report.indexable_items, 0);
    let rendered = body(&report);
    assert!(!rendered.contains("bank-a"), "{rendered}");
}

/// A whole-store scope (`None`) still lists every bank, the behavior callers
/// had before the scope parameter existed.
#[test]
fn whole_store_scope_lists_every_bank() {
    let store = two_banks();
    let report = diagnose_scoped(
        &DiagnoseArgs {
            store: &store,
            index: &store,
            embedder: None,
            timestamp: TS,
        },
        None,
    );
    let mut banks = report.banks.clone();
    banks.sort();
    assert_eq!(banks, vec!["bank-a".to_owned(), "bank-b".to_owned()]);
    // Every bank's rows are counted: one plus two items, one plus two jobs.
    assert_eq!(report.indexable_items, 3);
    assert_eq!(report.pending_jobs, 3);
    assert!(report.ok, "{:?}", report.findings);
}

/// An ops store whose bank inventory read fails, so the scoped catalog finding
/// is exercised.
struct BlindOps;

impl OpsStore for BlindOps {
    fn ping(&self) -> Result<(), AmError> {
        Ok(())
    }

    fn schema_version(&self) -> Result<Option<String>, AmError> {
        Ok(Some(SCHEMA_VERSION.to_owned()))
    }

    fn embedding_space(&self) -> Result<EmbeddingSpace, AmError> {
        Ok(EmbeddingSpace::new(DEFAULT_EMBEDDING_MODEL, EMBEDDING_DIMS).expect("space"))
    }

    fn bank_ids(&self) -> Result<Vec<String>, AmError> {
        Err(failed("catalog unreadable"))
    }

    fn bank_exists(&self, _bank_id: &str) -> Result<bool, AmError> {
        Err(failed("catalog unreadable"))
    }

    fn register_bank(&self, _bank_id: &str, _created_at: &str) -> Result<(), AmError> {
        Ok(())
    }

    fn counts(&self) -> Result<clio_store::StoreCounts, AmError> {
        Ok(clio_store::StoreCounts::default())
    }

    fn integrity_signals(&self, _bank: Option<&str>) -> Result<IntegritySignals, AmError> {
        Ok(IntegritySignals::default())
    }

    fn drop_orphan_assoc_edges(&self, _bank: &str) -> Result<u64, AmError> {
        Ok(0)
    }
}

/// A failed bank inventory under a scope is reported against that bank, not
/// against the whole store, and lists no bank.
#[test]
fn unreadable_bank_inventory_is_scoped_to_the_bank() {
    let index = sqlite();
    let ops = BlindOps;
    let report = diagnose_scoped(
        &DiagnoseArgs {
            store: &ops,
            index: &index,
            embedder: None,
            timestamp: TS,
        },
        Some("bank-a"),
    );
    assert!(!report.ok);
    assert!(report.banks.is_empty(), "{:?}", report.banks);
    let unreadable: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.code == codes::SYSTEM_TABLES_UNREADABLE)
        .collect();
    assert_eq!(unreadable.len(), 1, "{:?}", report.findings);
    assert_eq!(unreadable[0].severity, Severity::Error);
    assert_eq!(unreadable[0].scope, "bank:bank-a");
}
