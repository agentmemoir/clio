// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the peer-fact tests of the scope ledger:
//! the peers-disabled not-applicable fact, the acknowledgement state a caller
//! supplies from the durable per-peer rows, the binding rule that keeps an
//! unbound or foreign row from settling a peer, the evidence rule that a
//! recorded removal is proven only by its proof reference, and the rule that
//! supplying those rows changes no other ledger row.
//!
//! ## Owns
//! - The enabled-peers row assertions for the supplied enablement fact and the
//!   durable per-peer action rows read back from the fixture store
//! - The unproven-removal negative and its proven positive over that same row
//!
//! ## Does not own
//! - The ledger itself (`inventory.rs`) and its peer rendering
//!   (`inventory_peer_facts.rs`)
//! - The durable per-peer row contract (`clio-store`'s `peer_actions`)
//!
//! ## Boundary
//!
//! Tests here break when an unbound, foreign, or unproven durable row settles
//! or disposes the ledger's peers row.
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the enabled-peers ledger row.

use super::{InventoryRow, ScopeInventory, reason, row_for, scope_inventory};
use crate::inventory_fixture::LedgerFixture;
use crate::subject_peer::{
    begin_denial, mark_acknowledged, mark_delivered, mark_refused, record_disposition,
};
use crate::test_unique::unique;
use clio_store::{PeerActionDisposition, PeerActionStore};

/// The fixture's ledger input with the durable rows of `request_id` read back
/// from the fixture store and supplied as the peer acknowledgement fact.
fn input_with_rows(fixture: &LedgerFixture, request_id: &str) -> super::InventoryInput {
    let mut input = fixture.input(Vec::new(), Some(true));
    input.peer_actions = fixture
        .store
        .load_for_request(request_id)
        .expect("read the fixture's durable peer rows back");
    input
}

/// Every ledger row except the enabled-peers row, in report order.
fn other_rows(ledger: &ScopeInventory) -> Vec<InventoryRow> {
    ledger
        .rows
        .iter()
        .filter(|row| row.family != "enabled_peers")
        .cloned()
        .collect()
}

#[test]
fn a_peers_disabled_deployment_records_not_applicable() {
    let fixture = LedgerFixture::create();
    let ledger = scope_inventory(&fixture.input(Vec::new(), Some(false)));

    let row = row_for(&ledger, "enabled_peers");
    assert!(
        !row.open && row.open_reason.is_none(),
        "peers disabled is a known fact, not an open unknown: {:?}",
        row.open_reason
    );
    assert!(row.gap.is_none(), "no acknowledgement gap applies");
    assert!(
        row.source.contains("not applicable"),
        "the disabled mode is recorded as not applicable: {}",
        row.source
    );
    assert!(
        !row.source.contains("not stored"),
        "a known configuration fact is never reported as not stored: {}",
        row.source
    );
}

#[test]
fn a_supplied_acknowledged_peer_is_settled_from_its_durable_row() {
    let fixture = LedgerFixture::create();
    let request = unique("inv-peer-ack");
    let peer = unique("inv-peer-ack-a");
    begin_denial(&fixture.store, &request, &peer).expect("begin the peer's denial row");
    mark_acknowledged(&fixture.store, &request, &peer, "gen-1:hash-a:salt-a")
        .expect("record the peer's durable acknowledgement");

    let ledger = scope_inventory(&input_with_rows(&fixture, &request));
    let row = row_for(&ledger, "enabled_peers");
    assert!(
        !row.open && row.open_reason.is_none(),
        "an acknowledged peer leaves nothing open: {:?}",
        row.open_reason
    );
    assert!(row.gap.is_none(), "no acknowledgement gap applies");
    assert!(
        row.source.contains(&format!("{peer} acknowledged")),
        "the durable acknowledgement is named: {}",
        row.source
    );
    assert!(
        !row.source.contains("not stored"),
        "a durable record is stored: {}",
        row.source
    );
}

#[test]
fn a_delivered_peer_stays_open_and_is_reported_pending() {
    let fixture = LedgerFixture::create();
    let request = unique("inv-peer-pending");
    let peer = unique("inv-peer-pending-a");
    begin_denial(&fixture.store, &request, &peer).expect("begin the peer's denial row");
    mark_delivered(&fixture.store, &request, &peer).expect("record the peer's delivery");

    let ledger = scope_inventory(&input_with_rows(&fixture, &request));
    let row = row_for(&ledger, "enabled_peers");
    assert!(row.open, "delivery without proof keeps the peer open");
    assert!(
        row.source
            .contains(&format!("{peer} pending acknowledgement")),
        "the delivered peer is reported pending: {}",
        row.source
    );
    let why = reason(&ledger, "enabled_peers");
    assert!(
        why.contains(&peer) && why.contains("pending acknowledgement"),
        "the open reason names the pending peer: {why}"
    );
}

#[test]
fn a_refused_peer_without_a_disposition_stays_open_and_is_named() {
    let fixture = LedgerFixture::create();
    let request = unique("inv-peer-refused");
    let peer = unique("inv-peer-refused-a");
    begin_denial(&fixture.store, &request, &peer).expect("begin the peer's denial row");
    mark_refused(&fixture.store, &request, &peer).expect("record the peer's refusal");

    let ledger = scope_inventory(&input_with_rows(&fixture, &request));
    let row = row_for(&ledger, "enabled_peers");
    assert!(row.open, "a refusal with no disposition stays open");
    assert!(
        row.source
            .contains(&format!("{peer} refused, no disposition recorded")),
        "the refusal is named: {}",
        row.source
    );
    let why = reason(&ledger, "enabled_peers");
    assert!(
        why.contains(&peer) && why.contains("no disposition recorded"),
        "the open reason names the refusal: {why}"
    );
}

#[test]
fn a_refused_peer_with_an_evidenced_disposition_is_never_read_as_acknowledged() {
    let fixture = LedgerFixture::create();
    let request = unique("inv-peer-retention");
    let peer = unique("inv-peer-retention-a");
    begin_denial(&fixture.store, &request, &peer).expect("begin the peer's denial row");
    record_disposition(
        &fixture.store,
        &request,
        &peer,
        PeerActionDisposition::RetentionApproved,
        Some("retention-policy-1"),
    )
    .expect("record the peer's evidenced retention disposition");

    let ledger = scope_inventory(&input_with_rows(&fixture, &request));
    let row = row_for(&ledger, "enabled_peers");
    assert!(
        row.source
            .contains(&format!("{peer} disposed (retention approved)")),
        "the recorded disposition is named: {}",
        row.source
    );
    assert!(
        !row.source.contains("acknowledged"),
        "a retention is never read as an acknowledgement: {}",
        row.source
    );
    assert!(
        !row.open && row.open_reason.is_none(),
        "an evidenced disposition is recorded, not open: {:?}",
        row.open_reason
    );
}

#[test]
fn an_unproven_removal_evidence_row_stays_open_and_is_never_disposed() {
    let fixture = LedgerFixture::create();
    let request = unique("inv-peer-unproven");
    let peer = unique("inv-peer-unproven-a");
    begin_denial(&fixture.store, &request, &peer).expect("begin the peer's denial row");
    record_disposition(
        &fixture.store,
        &request,
        &peer,
        PeerActionDisposition::RemovalEvidence,
        None,
    )
    .expect("record the peer's unproven removal");

    let ledger = scope_inventory(&input_with_rows(&fixture, &request));
    let row = row_for(&ledger, "enabled_peers");
    assert!(
        row.open,
        "a removal the durable row cannot prove keeps the peer open"
    );
    assert!(
        row.source
            .contains(&format!("{peer} pending, removal evidence is unproven")),
        "the unproven removal is named pending: {}",
        row.source
    );
    assert!(
        !row.source.contains("disposed"),
        "an unproven removal is never reported disposed: {}",
        row.source
    );
    let why = reason(&ledger, "enabled_peers");
    assert!(
        why.contains(&peer) && why.contains("unproven"),
        "the open reason names the unproven removal: {why}"
    );
}

#[test]
fn a_proven_removal_evidence_row_is_reported_disposed() {
    let fixture = LedgerFixture::create();
    let request = unique("inv-peer-proven");
    let peer = unique("inv-peer-proven-a");
    begin_denial(&fixture.store, &request, &peer).expect("begin the peer's denial row");
    record_disposition(
        &fixture.store,
        &request,
        &peer,
        PeerActionDisposition::RemovalEvidence,
        Some("deletion-9"),
    )
    .expect("record the peer's proven removal");

    let ledger = scope_inventory(&input_with_rows(&fixture, &request));
    let row = row_for(&ledger, "enabled_peers");
    assert!(
        row.source
            .contains(&format!("{peer} disposed (removal evidence)")),
        "the proven removal is named disposed: {}",
        row.source
    );
    assert!(
        !row.open && row.open_reason.is_none(),
        "a proven removal settles the peer: {:?}",
        row.open_reason
    );
    assert!(row.gap.is_none(), "no acknowledgement gap applies");
}

#[test]
fn an_action_id_that_is_not_the_derived_one_never_settles_the_peer() {
    let fixture = LedgerFixture::create();
    let request = unique("inv-peer-unbound");
    let peer = unique("inv-peer-unbound-a");
    begin_denial(&fixture.store, &request, &peer).expect("begin the peer's denial row");
    mark_acknowledged(&fixture.store, &request, &peer, "gen-3:hash-c:salt-c")
        .expect("record the peer's durable acknowledgement");
    let mut input = input_with_rows(&fixture, &request);
    input.peer_actions[0].action_id = "other:key".to_owned();

    let ledger = scope_inventory(&input);
    let row = row_for(&ledger, "enabled_peers");
    assert!(
        row.open,
        "an action id that is not the derived one settles nothing"
    );
    assert!(
        !row.source.contains("acknowledged"),
        "an unbound row is never reported acknowledged: {}",
        row.source
    );
    let why = reason(&ledger, "enabled_peers");
    assert!(
        why.contains(&peer) && why.contains("not bound to the derived action id"),
        "the open reason names the unbound peer: {why}"
    );
}

#[test]
fn a_row_for_another_request_never_settles_the_peer() {
    let fixture = LedgerFixture::create();
    let request = unique("inv-peer-foreign-req");
    let peer = unique("inv-peer-foreign-req-a");
    begin_denial(&fixture.store, &request, &peer).expect("begin the peer's denial row");
    mark_acknowledged(&fixture.store, &request, &peer, "gen-4:hash-d:salt-d")
        .expect("record the peer's durable acknowledgement");
    let mut input = input_with_rows(&fixture, &request);
    input.peer_actions[0].request_id = unique("inv-peer-other-req");

    let ledger = scope_inventory(&input);
    let row = row_for(&ledger, "enabled_peers");
    assert!(row.open, "a row for another request settles nothing");
    assert!(
        !row.source.contains("acknowledged"),
        "a foreign-request row is never reported acknowledged: {}",
        row.source
    );
    let why = reason(&ledger, "enabled_peers");
    assert!(
        why.contains(&peer) && why.contains("not bound to the derived action id"),
        "the open reason names the foreign row's peer: {why}"
    );
}

#[test]
fn a_row_for_another_peer_never_settles_the_peer() {
    let fixture = LedgerFixture::create();
    let request = unique("inv-peer-foreign-peer");
    let peer = unique("inv-peer-foreign-peer-a");
    begin_denial(&fixture.store, &request, &peer).expect("begin the peer's denial row");
    mark_acknowledged(&fixture.store, &request, &peer, "gen-5:hash-e:salt-e")
        .expect("record the peer's durable acknowledgement");
    let mut input = input_with_rows(&fixture, &request);
    input.peer_actions[0].peer_id = unique("inv-peer-other");

    let ledger = scope_inventory(&input);
    let row = row_for(&ledger, "enabled_peers");
    assert!(row.open, "a row for another peer settles nothing");
    assert!(
        !row.source.contains("acknowledged"),
        "a foreign-peer row is never reported acknowledged: {}",
        row.source
    );
    let why = reason(&ledger, "enabled_peers");
    assert!(
        why.contains("not bound to the derived action id"),
        "the open reason names the unbound row: {why}"
    );
}

#[test]
fn supplying_the_durable_peer_rows_changes_no_other_row() {
    let fixture = LedgerFixture::create();
    let request = unique("inv-peer-scope");
    let peer = unique("inv-peer-scope-a");
    begin_denial(&fixture.store, &request, &peer).expect("begin the peer's denial row");
    mark_acknowledged(&fixture.store, &request, &peer, "gen-2:hash-b:salt-b")
        .expect("record the peer's durable acknowledgement");
    let enabled_only = scope_inventory(&fixture.input(Vec::new(), Some(true)));
    let with_rows = scope_inventory(&input_with_rows(&fixture, &request));

    assert_eq!(
        other_rows(&enabled_only),
        other_rows(&with_rows),
        "the peer fact changes only the enabled-peers row"
    );
    assert_ne!(
        row_for(&enabled_only, "enabled_peers"),
        row_for(&with_rows, "enabled_peers"),
        "the peers row itself moves from the enabled fact to the durable record"
    );
}
