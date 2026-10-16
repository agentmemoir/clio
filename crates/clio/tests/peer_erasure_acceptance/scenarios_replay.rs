// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the reconnect and duplicate-delivery
//! acceptance scenarios: the offline peer that reconnects and is denied before
//! it serves or applies, and the repeated and concurrent delivery whose
//! restart resumes reconciliation without a repeated purge.
//!
//! ## Owns
//! - `scenario_02` and `scenario_03`
//! - Their shared reconnect, stale-replay, concurrent-delivery, and
//!   settled-peer assertions
//!
//! ## Does not own
//! - The coordinator fixture (`run`), the driver calls (`connect`), the peer
//!   process (`sync_wire_support::peer`), and the probes (`support`)
//! - The frozen-set restart scenario and its shared helpers
//!   (`scenarios_peer`)
//! - Test registration on either backend (`sqlite`, `postgres`)
//!
//! ## Boundary
//!
//! Changes to what these two scenarios assert should normally require changes
//! here. Changes to the fixtures they drive should not.
//!
//! Keep this module focused on the reconnect and duplicate-delivery scenarios.

#![allow(dead_code)]

use clio::ErrorCode;
use clio::store::{LocalEventState, PeerActionDisposition, PeerActionState};
use clio_compliance::{ErasureReconcileReport, mark_acknowledged, mark_delivered};
use clio_sync::delivery::DeliveryOutcome;
use clio_sync::denial::DenialProof;
use std::sync::Arc;

use super::connect::{deliver_owned, pull_into, push_pending};
use super::nodes::Node;
use super::run::{Aux, Run, peer_action_id};
use super::scenarios_peer::{
    deliver, deliver_and_acknowledge, delivered, open_request, proof_ref, record_intent,
    resume_closure, seed_subject_and_control,
};
use super::support::{
    AcceptanceStore, BANK, BANKS, CONTROL_BANK, CONTROL_SUBJECT, SUBJECT, write_item,
};

/// Resume the offline peer, deliver the denial, prove the peer recorded it
/// durably, and assert a repeat delivery re-proves the same record without
/// writing a second one.
fn reconnect_and_prove_denial<T: AcceptanceStore + 'static>(
    run: &Run<T>,
    peer: &Node,
    request_id: &str,
) -> DenialProof {
    peer.resume().expect("resume the peer");
    let proof = deliver_and_acknowledge(run, peer, request_id);
    let (_, denials, fingerprint) = peer
        .journal_facts()
        .expect("the reconnected peer records the denial durably");
    assert!(denials >= 1, "denials {denials}");
    assert_eq!(fingerprint.as_deref(), Some(run.fingerprint.as_str()));
    let repeat = deliver(run, peer, request_id);
    assert_eq!(
        delivered(&repeat),
        &proof,
        "a repeat delivery re-proves the same durable record"
    );
    let (_, denials_after_repeat, _) = peer.journal_facts().expect("journal");
    assert_eq!(denials_after_repeat, denials, "no second record is written");
    proof
}

/// Push one stale writer's pending mutations to the peer and assert the denied
/// bank's content never becomes readable there and no subject key is minted
/// for it: the stale create is quarantined under the denial's own reason.
fn stale_replay_is_refused<T: AcceptanceStore + 'static>(stale: &Aux<T>, peer: &Node) {
    push_pending(&stale.store, stale.device(), peer.remote(), &BANKS)
        .expect("the push completes with the stale create refused");
    let letters = peer.store.dead_letters();
    let reasons: Vec<&str> = letters
        .iter()
        .map(|letter| letter.reason_code.as_str())
        .collect();
    assert!(
        reasons.contains(&"erased_subject"),
        "the stale create is quarantined: {reasons:?}"
    );
    assert_eq!(
        peer.store.subject_items(BANK, SUBJECT),
        0,
        "the stale create never becomes readable at the peer"
    );
    assert_eq!(
        peer.store.subject_keys(SUBJECT),
        0,
        "no subject key is minted for the denied subject"
    );
}

/// The denied bank refuses a pull through the client while the unrelated bank
/// still serves and its control subject reaches the reader's store.
fn denied_bank_refuses_while_control_serves<T: AcceptanceStore + 'static>(
    reader: &Aux<T>,
    peer: &Node,
) {
    let denied = pull_into(&reader.store, reader.device(), peer.remote(), &[BANK])
        .expect_err("a bank a recorded denial covers refuses the pull");
    assert_eq!(
        denied.code,
        ErrorCode::Forbidden,
        "the client refuses a 403 pull: {denied}"
    );
    assert!(
        denied.message.contains("erased_subject"),
        "the serve refusal names the erased subject: {denied}"
    );
    pull_into(
        &reader.store,
        reader.device(),
        peer.remote(),
        &[CONTROL_BANK],
    )
    .expect("the unrelated bank still serves");
    assert!(
        reader
            .store
            .get_memory_item(CONTROL_BANK, "s02-control")
            .expect("the control is readable")
            .is_some(),
        "the control subject reaches the reader"
    );
}

/// Two concurrent deliveries of the same denial prove one durable record, a
/// repeat delivery re-proves that same record, and neither writes a second
/// one: the peer's journal generation and denial count stay put.
fn concurrent_deliveries_prove_once<T: AcceptanceStore + 'static>(
    run: &Run<T>,
    peer: &Node,
    request_id: &str,
) -> DenialProof {
    let race = |run: &Run<T>, peer: &str, remote: &str| {
        let store = Arc::clone(&run.store);
        let device = run.device.clone();
        let remote = remote.to_owned();
        let (request, peer, subject, fingerprint) = (
            request_id.to_owned(),
            peer.to_owned(),
            SUBJECT.to_owned(),
            run.fingerprint.clone(),
        );
        std::thread::spawn(move || {
            deliver_owned(
                store,
                &device,
                &remote,
                &request,
                &peer,
                &subject,
                &fingerprint,
            )
        })
    };
    let first = race(run, peer.device_id(), peer.remote());
    let second = race(run, peer.device_id(), peer.remote());
    let first = first.join().expect("the first delivery thread joins");
    let second = second.join().expect("the second delivery thread joins");
    let first = delivered(&first.expect("the first delivery is well formed")).clone();
    let second = delivered(&second.expect("the second delivery is well formed")).clone();
    assert_eq!(
        first, second,
        "both deliveries prove the same durable record"
    );
    let (generation, denials) = {
        let (generation, denials, _) = peer.journal_facts().expect("the peer recorded it");
        (generation, denials)
    };
    let third = delivered(&deliver(run, peer, request_id)).clone();
    assert_eq!(third, first, "the repeat re-proves the same record");
    let (generation_repeat, denials_repeat, _) = peer.journal_facts().expect("journal");
    assert_eq!(
        (generation_repeat, denials_repeat),
        (generation, denials),
        "a repeated delivery writes no second record"
    );
    first
}

/// Assert the reconcile pass settled `peer`'s action and wrote exactly one
/// allowed event row for it.
fn assert_settled_peer<T: AcceptanceStore + 'static>(
    run: &Run<T>,
    report: &ErasureReconcileReport,
    request_id: &str,
    peer: &str,
) {
    let action = peer_action_id(request_id, peer);
    assert!(report.settled.contains(&action), "{report:?}");
    let rows = run.events_for(&action);
    assert_eq!(rows.len(), 1, "one durable event row per peer/action");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
}

/// T02: reconnect the offline peer with old subject creates queued; the denial
/// applies before serving or applying, the erased content stays absent, and
/// the other subject works before and after reconciliation.
pub fn scenario_02<T: AcceptanceStore + 'static>(run: &mut Run<T>, a: &Node, b: &Node) {
    let request_id = open_request(run, "s02", &[a, b]);
    deliver_and_acknowledge(run, a, &request_id);

    b.stop().expect("stop the second peer");
    let offline = deliver(run, b, &request_id);
    assert!(
        matches!(offline, DeliveryOutcome::Unreachable { .. }),
        "an offline peer yields no durable answer: {offline:?}"
    );

    seed_subject_and_control(run, "s02");
    let erased = run.erase(&request_id).expect("the local effect runs");
    assert_eq!(
        erased.status, "PROCESSING",
        "the offline peer's result stays pending"
    );
    let tombstones = run.tombstones(&request_id);

    // Reconnect: the denial is delivered and durably proven first.
    reconnect_and_prove_denial(run, b, &request_id);

    // Old subject creates queued while offline, replayed over the wire.
    let stale = run.aux("s02-stale");
    write_item(stale.store.as_ref(), "s02-stale-a", BANK, SUBJECT);
    write_item(
        stale.store.as_ref(),
        "s02-control",
        CONTROL_BANK,
        CONTROL_SUBJECT,
    );
    stale_replay_is_refused(&stale, b);
    assert_eq!(
        b.store.subject_items(CONTROL_BANK, CONTROL_SUBJECT),
        1,
        "the positive control applies"
    );

    // Reads: the denied bank refuses while the other subject serves.
    let reader = run.aux("s02-reader");
    denied_bank_refuses_while_control_serves(&reader, b);
    assert!(
        !b.logs().trim().is_empty(),
        "the peer's captured process log is non-empty"
    );

    // Reconciliation settles both peer actions.
    let resume = resume_closure();
    let report = run.reconcile(&resume).expect("the pass settles the peers");
    for peer in [a.device_id(), b.device_id()] {
        assert_settled_peer(run, &report, &request_id, peer);
        let rows = run.events_for(&peer_action_id(&request_id, peer));
        assert!(rows[0].result.is_some(), "the outcome carries its result");
    }

    // After reconciliation the stale replay is still refused and the control
    // still applies; the completed purge is not repeated.
    write_item(stale.store.as_ref(), "s02-late-b", BANK, SUBJECT);
    stale_replay_is_refused(&stale, b);
    pull_into(&reader.store, reader.device(), b.remote(), &[BANK])
        .expect_err("the denied bank still refuses after reconciliation");
    let settled = run.erase(&request_id).expect("both peers are settled");
    assert_eq!(settled.status, "COMPLETED");
    assert_eq!(
        run.tombstones(&request_id),
        tombstones,
        "the completed purge is not repeated"
    );
}

/// T03: repeat and race the denial delivery, restart after the peer's denial
/// but before the coordinator acknowledgement; one durable result per
/// peer/action and a resume that repeats no purge.
pub fn scenario_03<T: AcceptanceStore + 'static>(run: &mut Run<T>, a: &Node, b: &Node) {
    let request_id = open_request(run, "s03", &[a, b]);
    record_intent(run, &request_id, a);
    record_intent(run, &request_id, b);

    // Two concurrent deliveries of the same denial.
    let first = concurrent_deliveries_prove_once(run, a, &request_id);

    let proof_b = delivered(&deliver(run, b, &request_id)).clone();
    mark_delivered(run.store(), &request_id, a.device_id()).expect("the delivery records");
    mark_delivered(run.store(), &request_id, b.device_id()).expect("the delivery records");
    for peer in [a.device_id(), b.device_id()] {
        assert_eq!(
            run.peer_row(&request_id, peer).state,
            PeerActionState::Delivered
        );
    }

    seed_subject_and_control(run, "s03");
    let erased = run.erase(&request_id).expect("the effect runs");
    assert_eq!(
        erased.status, "PROCESSING",
        "a delivered but unacknowledged peer refuses completion"
    );
    let tombstones = run.tombstones(&request_id);

    // The restart happens after the peer's denial, before the coordinator
    // acknowledgement.
    run.restart();
    for peer in [a.device_id(), b.device_id()] {
        let row = run.peer_row(&request_id, peer);
        assert_eq!(
            row.state,
            PeerActionState::Delivered,
            "the delivered row restarts verbatim"
        );
        assert_eq!(row.disposition, PeerActionDisposition::Unknown);
    }
    let restarted = run.erase(&request_id).expect("the restarted attempt runs");
    assert_eq!(restarted.status, "PROCESSING");
    assert_eq!(
        run.tombstones(&request_id),
        tombstones,
        "the restart repeats no purge"
    );

    // The durable acknowledgements arrive after the restart; reconciliation
    // settles each peer action exactly once.
    mark_acknowledged(run.store(), &request_id, a.device_id(), &proof_ref(&first))
        .expect("the acknowledgement records");
    mark_acknowledged(
        run.store(),
        &request_id,
        b.device_id(),
        &proof_ref(&proof_b),
    )
    .expect("the acknowledgement records");
    let resume = resume_closure();
    let report = run
        .reconcile(&resume)
        .expect("the pass settles both peer actions");
    for peer in [a.device_id(), b.device_id()] {
        assert_settled_peer(run, &report, &request_id, peer);
    }
    let action_a = peer_action_id(&request_id, a.device_id());
    let again = run.reconcile(&resume).expect("the second pass runs");
    assert!(
        !again.settled.contains(&action_a),
        "a settled peer action is never settled twice"
    );
    assert_eq!(
        run.events_for(&action_a).len(),
        1,
        "no second outcome is written"
    );

    let settled = run.erase(&request_id).expect("both peers acknowledged");
    assert_eq!(settled.status, "COMPLETED");
    assert_eq!(
        run.tombstones(&request_id),
        tombstones,
        "the completed purge is not repeated after the resume"
    );
    assert_eq!(
        run.peer_row(&request_id, a.device_id()).state,
        PeerActionState::Acknowledged,
        "the peer row is read, never rewritten"
    );
}
