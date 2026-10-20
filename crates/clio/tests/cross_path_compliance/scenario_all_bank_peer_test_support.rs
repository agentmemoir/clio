// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared state the all-bank and peer arms
//! build and measure: the seeded items in every bank that holds the denied
//! subject's data, the frozen request the production peer facts produce, and
//! the probes that read one item and one erasure tombstone back.
//!
//! ## Owns
//! - `PEER_BANK` and `PEER_ITEM`, the second bank's fixture
//! - `item_in` and `seed`, the seeded items
//! - `DerivedPeerSet`, `derive_peer_set`, `seed_request`, and
//!   `assert_recorded_peer_set`, the frozen peer set
//! - `item_read`, `item_hash`, and `erased_item_hashes`, the read-back probes
//!
//! ## Does not own
//! - The scenario arms and the claims they make (see `scenario_all_bank_peer`)
//! - The real deployment and the spawned peer (see `peer_process_test_support`)
//! - The store, ledger, and erasure primitives (see `clio-store` and
//!   `clio-compliance`)
//!
//! ## Boundary
//!
//! Changes to how the seeded state, the frozen peer set, or the read-back
//! probes are built should normally require changes here. Changes to what a
//! scenario claims should not.
//!
//! Keep this module focused on the shared state, with no tests.

use crate::case_fixtures::{
    BANK, CONTROL_ITEM, CONTROL_MARKER, CONTROL_SUBJECT, DENIED_ITEM, DENIED_MARKER, DENIED_SUBJECT,
};
use crate::cli_test_support::salt_bytes;
use clio::store::{ComplianceStore, DurableRequestStore, PeerActionStore, Store};
use clio::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use clio_compliance::subject_request_store_wiring::{record_to_row, row_to_record};
use clio_compliance::{
    PeerEnablement, RequestCommand, RequestKind, RequestScope, SubjectRequestLedger,
    derive_peer_enablement, snapshot_peers,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;

/// The second bank that shares the denied subject's DEK.
pub const PEER_BANK: &str = "cross-path-peer-bank";
/// The denied subject's item in [`PEER_BANK`].
pub const PEER_ITEM: &str = "cross-path-peer-item";

/// One synthetic item carrying `subject` in `bank`.
fn item_in(bank: &str, id: &str, subject: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.to_owned(),
        bank_id: bank.to_owned(),
        subject_id: subject.to_owned(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot: Some(json!({"entity": "synthetic cross-path subject"})),
        gist: Some(gist.to_owned()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// Seed the denied subject's item in each sharing bank, and the control item.
pub fn seed<S: Store>(store: &S) {
    for (bank, id, subject, gist) in [
        (BANK, DENIED_ITEM, DENIED_SUBJECT, DENIED_MARKER),
        (
            PEER_BANK,
            PEER_ITEM,
            DENIED_SUBJECT,
            "marker-peer-cross-path",
        ),
        (BANK, CONTROL_ITEM, CONTROL_SUBJECT, CONTROL_MARKER),
    ] {
        store
            .create_memory_item(&item_in(bank, id, subject, gist), "harness")
            .expect("write the synthetic item");
    }
}

/// The peer facts the production derives from one deployment's credential
/// store: the enablement the request records, the peer labels it names, and
/// the banks those peers cover.
pub struct DerivedPeerSet {
    /// Whether the deployment needs peer work at all.
    pub enablement: PeerEnablement,
    /// The enabled peer labels, in the credential store's order.
    pub labels: Vec<String>,
    /// The banks the enabled peers cover, sorted.
    pub banks: BTreeSet<String>,
}

/// The peer facts the production derives from the credential store at
/// `credential_store`.
///
/// The derivation is the one the request-start path runs: the same
/// enablement rule, the same label list, and the same bank set, read from
/// the same credential store the peer wrote. The single-host fact is fixed
/// to `false`, because the deployment under test runs a peer process, so its
/// peer set is not the not-applicable one.
///
/// # Panics
/// The credential store cannot be read, or it holds a record the production
/// refuses.
#[must_use]
pub fn derive_peer_set(credential_store: &Path) -> DerivedPeerSet {
    let path = credential_store.to_string_lossy().into_owned();
    let (enablement, labels, banks) = derive_peer_enablement(
        &Value::Null,
        &|key: &str| (key == clio_config::auth_tokens::AUTH_TOKEN_FILE_ENV).then_some(path.clone()),
        false,
    )
    .expect("derive the peer set from the credential store");
    DerivedPeerSet {
        enablement,
        labels,
        banks,
    }
}

/// Persist an in-progress erasure request over the denied subject, with the
/// production-derived peer set and peer banks, and snapshot its peer rows.
pub fn seed_request<S: DurableRequestStore + PeerActionStore>(
    store: &S,
    request_id: &str,
    derived: &DerivedPeerSet,
) {
    let scope = RequestScope::Subject {
        bank_id: BANK.to_owned(),
        subject_id: DENIED_SUBJECT.to_owned(),
    };
    let peer_refs: Vec<&str> = derived.labels.iter().map(String::as_str).collect();
    let bank_refs: Vec<&str> = derived.banks.iter().map(String::as_str).collect();
    let mut ledger = SubjectRequestLedger::new();
    ledger
        .register_with_enablement(
            request_id,
            RequestKind::Erasure,
            &scope,
            derived.enablement,
            &peer_refs,
            &bank_refs,
        )
        .expect("the erasure request registers");
    ledger
        .apply(request_id, &RequestCommand::Begin)
        .expect("the request starts");
    let record = ledger
        .record(request_id)
        .expect("the record exists")
        .clone();
    store
        .save(&record_to_row(&record))
        .expect("the record persists");
    snapshot_peers(store, &record).expect("the peer rows snapshot");
}

/// Assert the durable request record holds the production-derived peer set.
///
/// The record's enablement, peer banks, and peer labels are compared with
/// the derived ones by equality, so a record that names a bank or a peer the
/// production did not derive fails this check.
pub fn assert_recorded_peer_set<S: DurableRequestStore + ?Sized>(
    store: &S,
    request_id: &str,
    derived: &DerivedPeerSet,
) {
    let row = store
        .load(request_id)
        .expect("read the durable request row")
        .expect("the request row exists");
    let record = row_to_record(&row).expect("decode the durable request record");
    assert_eq!(
        record.peer_enablement, derived.enablement,
        "the recorded peer enablement is the production-derived fact"
    );
    assert_eq!(
        record.peer_banks, derived.banks,
        "the recorded peer banks are the production-derived set"
    );
    let mut recorded: Vec<String> = record.peers.keys().cloned().collect();
    let mut expected = derived.labels.clone();
    recorded.sort();
    expected.sort();
    assert_eq!(
        recorded, expected,
        "the recorded peer labels are the production-derived set"
    );
}

/// Read one item back from one bank: the item id when its content is
/// readable, the production's error code when it is not.
pub fn item_read<S: Store + ?Sized>(
    store: &S,
    bank: &str,
    item_id: &str,
) -> Result<Option<String>, String> {
    store
        .get_memory_item(bank, item_id)
        .map(|item| item.map(|item| item.id))
        .map_err(|error| error.code.as_str().to_owned())
}

/// The one-way hash the erasure tombstone records for one item id.
#[must_use]
pub fn item_hash(item_id: &str) -> String {
    clio::store::sha256_hex(&salt_bytes(), item_id)
}

/// The item ids the erasure tombstones of one request cover, as their hashes.
///
/// Only the tombstones of that request count, so the set names the items one
/// erasure erased and nothing else. A shredded key writes no tombstone, so
/// the set is the purge's own record.
pub fn erased_item_hashes<S: ComplianceStore + ?Sized>(
    store: &S,
    bank: &str,
    request_id: &str,
) -> BTreeSet<String> {
    store
        .list_tombstones(bank)
        .expect("read the bank's erasure tombstones")
        .iter()
        .filter(|tombstone| tombstone.request_id == request_id)
        .filter_map(|tombstone| tombstone.item_id_hash.clone())
        .collect()
}
