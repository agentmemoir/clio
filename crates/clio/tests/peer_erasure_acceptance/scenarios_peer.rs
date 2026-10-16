// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the offline-peer acceptance scenario over the
//! coordinator run and two spawned peer processes, together with the shared
//! delivery, intent, acknowledgement, and assertion helpers the sibling
//! scenario modules import. The scenario freezes the required peer set, takes
//! one peer offline, erases, and restarts the coordinator: the frozen set
//! survives, the offline result stays pending, and technical completion refuses
//! on both sides of the restart.
//!
//! ## Owns
//! - `scenario_01` and the shared helpers around it: request opening, denial
//!   intent recording, delivery and acknowledgement, pending-row assertions,
//!   the resume closure, and subject/control seeding
//!
//! ## Does not own
//! - `scenario_02` and `scenario_03` (`scenarios_replay`)
//! - The coordinator fixture (`run`), the driver calls (`connect`), the peer
//!   process (`sync_wire_support::peer`), and the probes (`support`)
//! - The removed-peer disposition and event-failure scenarios
//!   (`scenarios_disposition`)
//! - Test registration on either backend (`sqlite`, `postgres`)
//!
//! ## Boundary
//!
//! Changes to what `scenario_01` asserts, and to the shared helpers its sibling
//! modules import from here, should normally require changes here. Changes to
//! the scenarios `scenarios_replay` owns (`scenario_02`, `scenario_03`) should
//! not.
//!
//! Keep this module focused on the offline-peer acceptance scenario and its
//! shared helpers.

#![allow(dead_code)]

use clio::Actor;
use clio::store::{PeerActionDisposition, PeerActionState};
use clio_compliance::{
    ErasureResume, PeerState, SubjectRequestRecord, mark_acknowledged, mark_delivered,
    record_peer_denial_intent,
};
use clio_sync::delivery::DeliveryOutcome;
use clio_sync::denial::DenialProof;

use super::nodes::Node;
use super::run::{Run, peer_action_id};
use super::support::{AcceptanceStore, BANK, CONTROL_SUBJECT, NOW, SUBJECT, unique, write_item};
use crate::sync_wire_support::TOKEN;

/// Open one erasure request whose frozen required set is every `node`.
pub fn open_request<T: AcceptanceStore + 'static>(
    run: &Run<T>,
    tag: &str,
    nodes: &[&Node],
) -> String {
    let request_id = unique(tag);
    let labels: Vec<&str> = nodes.iter().map(|node| node.device_id()).collect();
    let record = run.open_request(&request_id, &labels);
    assert_eq!(
        record.peers.len(),
        labels.len(),
        "the frozen set is exactly the requested one"
    );
    request_id
}

/// Record the durable peer-denial intent one delivery requires first.
pub fn record_intent<T: AcceptanceStore + 'static>(run: &Run<T>, request_id: &str, node: &Node) {
    record_peer_denial_intent(
        run.store(),
        request_id,
        node.device_id(),
        Actor::Harness,
        NOW,
    )
    .expect("the denial intent is durable before delivery");
}

/// Record the intent and attempt one denial delivery to `node`.
pub fn deliver<T: AcceptanceStore + 'static>(
    run: &Run<T>,
    node: &Node,
    request_id: &str,
) -> DeliveryOutcome {
    record_intent(run, request_id, node);
    run.deliver(
        node.remote(),
        node.device_id(),
        request_id,
        SUBJECT,
        &run.fingerprint,
        Some(TOKEN),
    )
    .expect("the delivery call is well formed")
}

/// The durable proof one delivered outcome carries.
///
/// # Panics
/// The outcome is not a verified durable proof.
#[must_use]
pub fn delivered(outcome: &DeliveryOutcome) -> &DenialProof {
    match outcome {
        DeliveryOutcome::Delivered { proof } => proof,
        other => panic!("expected a verified durable denial proof, got {other:?}"),
    }
}

/// The stable proof reference one durable proof yields.
#[must_use]
pub fn proof_ref(proof: &DenialProof) -> String {
    format!("denial-proof-{}", proof.generation)
}

/// Record the delivery and the peer's durable acknowledgement of one verified
/// proof, returning the proof's reference.
pub fn acknowledge<T: AcceptanceStore + 'static>(
    run: &Run<T>,
    node: &Node,
    request_id: &str,
    proof: &DenialProof,
) -> String {
    mark_delivered(run.store(), request_id, node.device_id())
        .expect("the delivery is durable before its acknowledgement");
    let reference = proof_ref(proof);
    mark_acknowledged(run.store(), request_id, node.device_id(), &reference)
        .expect("the durable acknowledgement records");
    reference
}

/// Record the intent, deliver, and record the durable acknowledgement,
/// returning the verified proof.
pub fn deliver_and_acknowledge<T: AcceptanceStore + 'static>(
    run: &Run<T>,
    node: &Node,
    request_id: &str,
) -> DenialProof {
    let outcome = deliver(run, node, request_id);
    let proof = delivered(&outcome).clone();
    acknowledge(run, node, request_id, &proof);
    proof
}

/// Assert one peer's row is still the untouched pending snapshot.
pub fn assert_pending_row<T: AcceptanceStore + 'static>(
    run: &Run<T>,
    request_id: &str,
    node: &Node,
) {
    let row = run.peer_row(request_id, node.device_id());
    assert_eq!(
        row.state,
        PeerActionState::Pending,
        "an unanswered peer stays pending"
    );
    assert_eq!(row.disposition, PeerActionDisposition::Unknown);
    assert_eq!(
        row.proof_ref, None,
        "no proof is recorded without an answer"
    );
}

/// The resume closure every scenario over this run's subject supplies: the
/// run's own subject and the live bank set read before the effect.
pub fn resume_closure() -> impl Fn(&SubjectRequestRecord) -> Option<ErasureResume> {
    |_| {
        Some(ErasureResume {
            subject_id: SUBJECT.to_owned(),
            live_banks: vec![BANK.to_owned()],
        })
    }
}

/// Seed the erased subject and the positive control in the coordinator store.
pub fn seed_subject_and_control<T: AcceptanceStore + 'static>(run: &Run<T>, tag: &str) {
    write_item(run.store(), &format!("{tag}-subject"), BANK, SUBJECT);
    write_item(
        run.store(),
        &format!("{tag}-control"),
        BANK,
        CONTROL_SUBJECT,
    );
}

/// T01: freeze two required peers, take one offline, erase, restart the
/// coordinator; the same set survives, the offline result stays pending, and
/// technical completion refuses on both sides of the restart.
pub fn scenario_01<T: AcceptanceStore + 'static>(run: &mut Run<T>, a: &Node, b: &Node) {
    let request_id = open_request(run, "s01", &[a, b]);
    let frozen = run.peer_states(&request_id);
    assert_eq!(frozen.len(), 2, "two required peers are frozen: {frozen:?}");
    assert_eq!(frozen.get(a.device_id()), Some(&PeerState::Pending));
    assert_eq!(frozen.get(b.device_id()), Some(&PeerState::Pending));

    let proof = delivered(&deliver(run, a, &request_id)).clone();
    assert_eq!(proof.peer_id, a.device_id(), "the proof is the peer's own");
    assert_eq!(
        proof.salt_fingerprint, run.fingerprint,
        "the proof binds this deployment"
    );
    let (generation, denials, fingerprint) = a
        .journal_facts()
        .expect("the peer recorded the denial durably");
    assert!(
        generation > 0 && denials >= 1,
        "generation {generation}, denials {denials}"
    );
    assert_eq!(fingerprint.as_deref(), Some(run.fingerprint.as_str()));
    acknowledge(run, a, &request_id, &proof);
    assert_eq!(
        run.peer_row(&request_id, a.device_id()).state,
        PeerActionState::Acknowledged
    );

    // One peer goes offline: the delivery obtains no durable answer.
    b.stop().expect("stop the second peer");
    let offline = deliver(run, b, &request_id);
    assert!(
        matches!(offline, DeliveryOutcome::Unreachable { .. }),
        "an offline peer yields no durable answer: {offline:?}"
    );
    assert!(
        b.journal_facts().is_none(),
        "an offline peer records no denial"
    );
    assert_pending_row(run, &request_id, b);

    seed_subject_and_control(run, "s01");
    let first = run.erase(&request_id).expect("the local effect runs");
    assert_eq!(
        first.status, "PROCESSING",
        "an offline required peer refuses technical completion"
    );
    let tombstones = run.tombstones(&request_id);
    assert!(tombstones > 0, "the effect committed its purge");
    assert!(
        run.store()
            .get_memory_item(BANK, "s01-control")
            .expect("the control is readable")
            .is_some(),
        "the control subject survives the erasure"
    );

    // The coordinator restart: the frozen set and the pending result survive.
    let before = run.load_record(&request_id);
    run.restart();
    let after = run.load_record(&request_id);
    assert_eq!(
        after.peers, before.peers,
        "the frozen set survives the restart"
    );
    assert_pending_row(run, &request_id, b);
    assert_eq!(
        run.peer_row(&request_id, a.device_id()).state,
        PeerActionState::Acknowledged,
        "the acknowledged result survives the restart"
    );
    let restarted = run.erase(&request_id).expect("the restarted attempt runs");
    assert_eq!(
        restarted.status, "PROCESSING",
        "the restart still refuses completion"
    );
    assert_eq!(
        run.tombstones(&request_id),
        tombstones,
        "the restart repeats no purge"
    );

    // Reconciliation settles the acknowledged peer's action and keeps the
    // offline peer's action pending.
    let resume = resume_closure();
    let report = run.reconcile(&resume).expect("the restart pass runs");
    assert!(
        report
            .settled
            .contains(&peer_action_id(&request_id, a.device_id())),
        "the acknowledged peer settles: {report:?}"
    );
    assert!(
        report
            .pending
            .contains(&peer_action_id(&request_id, b.device_id())),
        "the offline peer stays pending: {report:?}"
    );
    assert_pending_row(run, &request_id, b);
    assert_eq!(
        run.tombstones(&request_id),
        tombstones,
        "reconciliation repeats no purge"
    );
}
