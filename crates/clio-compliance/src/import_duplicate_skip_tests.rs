// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the restore behaviour of a bundle row whose
//! text matches a stored row but whose identity differs: the row is reported
//! under `would_skip_duplicate` with the unconditional exact-duplicate reason,
//! never as a gate rejection.
//!
//! ## Owns
//! - The matching-text, different-identity restore skip
//!
//! ## Does not own
//! - General import planning (see `import_tests.rs`)
//! - Fixture builders (see `import_fixture.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use clio_admission::gated::{SignalSource, candidate_text};
use clio_store::{ItemInspectFilter, SqliteStore, Store};
use clio_types::{AmError, MemoryItem, OpContext};

use crate::bundle::ExportContentMode;
use crate::import::{ImportArgs, ImportReport, import};
use crate::import_fixture::{Harness, unique, write_bundle};
use crate::import_fixture_signals::EmptySignals;

/// Neighbor texts for one bank, read from the store the way the production
/// signal source reads them.
struct StoredTexts<'a>(&'a SqliteStore);

impl std::fmt::Debug for StoredTexts<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StoredTexts")
    }
}

impl SignalSource for StoredTexts<'_> {
    fn neighbor_texts(&self, item: &MemoryItem) -> Result<Vec<String>, AmError> {
        let mut texts = Vec::new();
        for row in self
            .0
            .inspect_items(&ItemInspectFilter::new(item.bank_id.as_str()))?
        {
            if let Some(stored) = self
                .0
                .get_memory_item(item.bank_id.as_str(), &row.item_id)?
            {
                texts.push(candidate_text(&stored));
            }
        }
        Ok(texts)
    }
}

fn import_restoring(
    h: &Harness,
    ctx: &OpContext,
    source: &str,
    signals: &StoredTexts<'_>,
) -> ImportReport {
    import(&ImportArgs {
        store: &h.store,
        batch: &h.store,
        persona: &h.store,
        history: &h.store,
        belief: &h.store,
        ctx,
        source,
        dry_run: false,
        force: false,
        reason: None,
        policy: h.policy(),
        signals,
        sink: &h.admit,
        belief_signals: &EmptySignals,
        timestamp: "2026-06-01T00:00:00Z",
    })
    .expect("import succeeds")
}

#[test]
fn a_restored_item_matching_stored_text_but_not_identity_is_skipped() {
    let h = Harness::new();
    let ctx = h.ctx();
    let signals = StoredTexts(&h.store);
    let entities = h.full_entities();
    let first = import_restoring(
        &h,
        &ctx,
        write_bundle(&entities, ExportContentMode::DsarPlaintext).path(),
        &signals,
    );

    let mut clone = entities.items[0].clone();
    clone.id = unique("dup-identity");
    let only = crate::bundle::ExportEntities {
        items: vec![clone],
        ..crate::bundle::ExportEntities::default()
    };
    let report = import_restoring(
        &h,
        &ctx,
        write_bundle(&only, ExportContentMode::DsarPlaintext).path(),
        &signals,
    );

    assert_eq!(report.would_skip_duplicate, 1);
    assert_eq!(report.would_skip, 0);
    assert_eq!(report.would_reject, 0);
    assert!(
        report
            .notes
            .iter()
            .any(|note| note.contains("exact_duplicate")),
        "{:?}",
        report.notes
    );

    // A row whose identity already exists is an ordinary idempotent skip, not a
    // duplicate-text drop, so the two counts stay separate.
    let same_identity = import_restoring(
        &h,
        &ctx,
        write_bundle(&entities, ExportContentMode::DsarPlaintext).path(),
        &signals,
    );
    assert_eq!(same_identity.would_skip, first.would_create);
    assert_eq!(same_identity.would_skip_duplicate, 0);
}
