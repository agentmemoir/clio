// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the operations-removal cascade when the
//! discarded id is the unit's hub: the hub is the unit's identity, so it stays
//! in the claim set and keeps its current-revision evidence entry, and the proof
//! count still describes the claim set, on both backends.
//!
//! ## Owns
//! - The discarded-hub cascade suite plus its per-backend runners
//!
//! ## Does not own
//! - The discarded-source cascade suite (`distilled_unit_discard_cascade_tests`)
//! - The evidence and claim-set rules themselves (`distilled_unit`)
//!
//! ## Boundary
//!
//! Changes to what a discarded hub does should normally update the suite here.
//! Changes to the discarded-source contract should not.
//!
//! Keep this module focused on the discarded-hub store contract.
//!
//! Test-only. Never compiled into production builds.

use super::distilled_unit_discard_cascade_tests::{CascadeBackend, seed_unit};
use crate::dek::{DekProvider, LocalDevKms};
use crate::distilled_unit::STALE_SOURCE_REMOVED;
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use crate::test_unique::unique;
use std::collections::BTreeSet;
use std::sync::Arc;

/// The backend-agnostic discarded-hub contract.
fn suite_discarded_hub(store: &dyn CascadeBackend) {
    let bank = unique("cascade-hub");
    let subject = unique("subject");
    let hub = unique("hub");
    let a = unique("source-a");
    let (unit_id, _first_record) = seed_unit(store, &bank, &subject, &hub, &[&a]);

    // The hub leaves active use. It is the unit's identity, not one of its
    // source ids, so the claim set keeps it.
    store
        .discard_item(&bank, &hub, "withdrawn", "2026-06-05T00:00:00Z", "tester")
        .unwrap();

    let after = store.get_distilled_unit(&bank, &unit_id).unwrap().unwrap();
    assert_eq!(after.source_ids, vec![a], "the hub is not a source id");
    assert_eq!(after.stale_reason.as_deref(), Some(STALE_SOURCE_REMOVED));
    assert_eq!(after.revision, 1, "the cascade writes no revision");
    assert!(after.claim_ids().contains(&hub), "the hub stays a claim");

    // The current revision's evidence follows the claim set, so the discarded
    // hub keeps its entry and the proof count still describes the claim set.
    let counted: BTreeSet<String> = after
        .current_evidence()
        .iter()
        .map(|entry| entry.source_id.clone())
        .collect();
    assert_eq!(counted, after.claim_ids(), "{counted:?}");
    assert!(
        counted.contains(&hub),
        "the discarded hub keeps its evidence entry"
    );
    assert_eq!(after.proof_count(), after.claim_ids().len());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_discarded_hub_stays_a_claim_and_keeps_its_evidence() {
        let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
        let store = SqliteStore::open_in_memory(kms).unwrap();
        suite_discarded_hub(&store);
    }

    #[test]
    fn postgres_discarded_hub_stays_a_claim_and_keeps_its_evidence() {
        let _guard = crate::lock_pg_test();
        let url = crate::db_url::postgres_url_from_env();
        let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
        let store = PostgresStore::open(url.as_str(), kms).unwrap();
        suite_discarded_hub(&store);
    }
}
