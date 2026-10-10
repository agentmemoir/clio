// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the test doubles used by the bundle import
//! fixtures: a neutral admission-signal source that reports no neighbors and no
//! prior, and a batch-boundary spy that records every staged `AuditEvent` while
//! delegating the write.
//!
//! ## Owns
//! - `EmptySignals` (item + belief neutral signal source)
//! - `AuditSpy` / `RecordedAudit` (observed `import_force` audit rows)
//!
//! ## Does not own
//! - The import harness and entity builders (`import_fixture`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use clio_admission::gated::SignalSource;
use clio_belief::BeliefSignalSource;
use clio_store::{BatchStore, BatchWrite, SqliteStore};
use clio_types::{AmError, MemoryItem};
use std::sync::Mutex;

#[derive(Debug)]
pub(super) struct EmptySignals;

impl SignalSource for EmptySignals {
    fn neighbor_texts(&self, _item: &MemoryItem) -> Result<Vec<String>, clio_types::AmError> {
        Ok(vec![])
    }
    fn prior_age_days(&self, _item: &MemoryItem) -> Result<Option<f64>, clio_types::AmError> {
        Ok(None)
    }
}

impl BeliefSignalSource for EmptySignals {
    fn neighbor_texts(
        &self,
        _bank_id: &str,
        _proposition: &str,
    ) -> Result<Vec<String>, clio_types::AmError> {
        Ok(vec![])
    }
    fn prior_age_days(
        &self,
        _bank_id: &str,
        _proposition: &str,
    ) -> Result<Option<f64>, clio_types::AmError> {
        Ok(None)
    }
}

/// One audit write staged through an import batch boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RecordedAudit {
    pub(super) operation: String,
    pub(super) item_id: Option<String>,
    pub(super) reason: Option<String>,
}

/// Test-only `BatchStore` double that records every staged `AuditEvent` and
/// then delegates the write to the wrapped store.
///
/// Persona rows have no ledger item, so their `import_force` rows are not
/// reachable through `audit_trail` (which anchors on an item or belief row):
/// the import tests observe them through this spy.
pub(super) struct AuditSpy<'a> {
    inner: &'a SqliteStore,
    audits: Mutex<Vec<RecordedAudit>>,
}

impl<'a> AuditSpy<'a> {
    pub(super) fn new(inner: &'a SqliteStore) -> Self {
        Self {
            inner,
            audits: Mutex::new(Vec::new()),
        }
    }

    /// Every audit write staged so far.
    pub(super) fn audits(&self) -> Vec<RecordedAudit> {
        self.audits.lock().expect("audit spy lock").clone()
    }
}

impl BatchStore for AuditSpy<'_> {
    fn apply_batch(&self, writes: &[BatchWrite]) -> Result<(), AmError> {
        {
            let mut audits = self.audits.lock().expect("audit spy lock");
            for write in writes {
                let BatchWrite::AuditEvent {
                    operation,
                    item_id,
                    detail,
                    ..
                } = write
                else {
                    continue;
                };
                audits.push(RecordedAudit {
                    operation: operation.clone(),
                    item_id: item_id.clone(),
                    reason: detail
                        .get("reason")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                });
            }
        }
        self.inner.apply_batch(writes)
    }
}
