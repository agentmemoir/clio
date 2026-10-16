// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the disposition, event-failure, and
//! invalid-result acceptance scenarios: the explicit removed-peer
//! dispositions, the injected intent and outcome failures whose restart and
//! prune leave the safety state intact, the peers-disabled core path, and the
//! invalid results that cannot acknowledge or widen scope.
//!
//! ## Owns
//! - `scenario_04`, `scenario_05`, `scenario_06_disabled`, and
//!   `scenario_06_invalid`
//!
//! ## Does not own
//! - The coordinator fixture (`run`), the driver calls (`connect`), the peer
//!   process (`sync_wire_support::peer`), and the probes (`support`)
//! - The offline, reconnect, and duplicate-delivery scenarios
//!   (`scenarios_peer`)
//! - Test registration on either backend (`sqlite`, `postgres`)
//!
//! ## Boundary
//!
//! Changes to what these scenarios assert should normally require changes
//! here. Changes to the fixtures they drive should not.
//!
//! Keep this module focused on the disposition, event-failure, and
//! invalid-result acceptance scenarios.

#![allow(dead_code)]

use clio::store::{LocalEventState, PeerActionDisposition, PeerActionState};
use clio::{Actor, ErrorCode};
use clio_compliance::{
    PeerEnablement, PeerState, mark_acknowledged, mark_delivered, mark_refused, record_disposition,
    record_peer_denial_intent,
};
use clio_sync::delivery::DeliveryOutcome;

use super::faults::FaultEvents;
use super::nodes::Node;
use super::run::{Run, erasure_action_id, peer_action_id};
use super::scenarios_peer::{
    assert_pending_row, deliver, deliver_and_acknowledge, delivered, open_request, proof_ref,
    resume_closure, seed_subject_and_control,
};
use super::support::{AcceptanceStore, BANK, NOW, SUBJECT};
use crate::sync_wire_support::TOKEN;

/// T04: a peer removed with the deletion evidence the peer itself proved
/// settles the request; an approved retention and an unknown disposition are
/// recorded but keep it open.
pub fn scenario_04<T: AcceptanceStore + 'static>(run: &mut Run<T>, a: &Node, b: &Node, c: &Node) {
    let request_id = open_request(run, "s04", &[a, b, c]);
    let proof = delivered(&deliver(run, a, &request_id)).clone();
    deliver(run, b, &request_id);
    deliver(run, c, &request_id);
    mark_delivered(run.store(), &request_id, b.device_id()).expect("the delivery records");
    mark_delivered(run.store(), &request_id, c.device_id()).expect("the delivery records");
    mark_acknowledged(run.store(), &request_id, a.device_id(), &proof_ref(&proof))
        .expect("the acknowledgement records");

    // The retention exception and the unknown disposition are recorded.
    let retained = record_disposition(
        run.store(),
        &request_id,
        b.device_id(),
        PeerActionDisposition::RetentionApproved,
        Some("retention-7"),
    )
    .expect("the approved retention records");
    assert_eq!(
        retained.disposition,
        PeerActionDisposition::RetentionApproved
    );
    assert_eq!(retained.proof_ref.as_deref(), Some("retention-7"));
    let unknown = mark_refused(run.store(), &request_id, c.device_id())
        .expect("the refusal records with no disposition");
    assert_eq!(unknown.state, PeerActionState::Refused);
    assert_eq!(unknown.disposition, PeerActionDisposition::Unknown);
    assert_eq!(unknown.proof_ref, None);

    seed_subject_and_control(run, "s04");
    let erased = run.erase(&request_id).expect("the effect runs");
    assert_eq!(
        erased.status, "PROCESSING",
        "a retention and an unknown disposition cannot yield full erasure"
    );
    assert!(run.tombstones(&request_id) > 0, "the local purge committed");

    let states = run.peer_states(&request_id);
    assert_eq!(
        states.get(a.device_id()),
        Some(&PeerState::Acknowledged),
        "the acknowledged peer settles"
    );
    assert_eq!(
        states.get(b.device_id()),
        Some(&PeerState::Pending),
        "an approved retention is not removal evidence"
    );
    assert_eq!(
        states.get(c.device_id()),
        Some(&PeerState::Pending),
        "an unknown disposition stays open"
    );
}

/// T04, deletion-evidence arm: the peer's own durable proof recorded as the
/// removal evidence settles the peer and the request without a second effect.
pub fn scenario_04_removal<T: AcceptanceStore + 'static>(run: &mut Run<T>, a: &Node) {
    let request_id = open_request(run, "s04-removal", &[a]);
    let proof = delivered(&deliver(run, a, &request_id)).clone();
    let evidence = proof_ref(&proof);
    let removed = record_disposition(
        run.store(),
        &request_id,
        a.device_id(),
        PeerActionDisposition::RemovalEvidence,
        Some(&evidence),
    )
    .expect("the removal evidence records");
    assert_eq!(removed.state, PeerActionState::Refused);
    assert_eq!(removed.disposition, PeerActionDisposition::RemovalEvidence);
    assert_eq!(removed.proof_ref.as_deref(), Some(evidence.as_str()));
    assert_eq!(
        run.peer_states(&request_id).get(a.device_id()),
        Some(&PeerState::Disposed),
        "a recorded removal evidence with its proof settles the peer"
    );

    seed_subject_and_control(run, "s04-removal");
    let erased = run.erase(&request_id).expect("the effect runs");
    assert_eq!(
        erased.status, "COMPLETED",
        "a recorded deletion evidence is full removal evidence"
    );
    let rows = run.events_for(&erasure_action_id(&request_id));
    assert_eq!(rows.len(), 1, "one durable result for the erasure action");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    assert!(
        run.peer_row(&request_id, a.device_id()).proof_ref.is_some(),
        "the row keeps the proof reference the operator recorded"
    );
}

/// T05: fail the peer-denial intent and then the outcome; the intent failure
/// has no effect, the restart reconciles the outcome once, and pruning
/// preserves the pending peer work and its denial safety state.
pub fn scenario_05<T: AcceptanceStore + 'static>(run: &mut Run<T>, a: &Node, b: &Node) {
    let request_id = open_request(run, "s05", &[a, b]);

    // The intent fails before any protected peer effect.
    let failed_intent = FaultEvents::intent_fails();
    let err = record_peer_denial_intent(
        &failed_intent,
        &request_id,
        b.device_id(),
        Actor::Harness,
        NOW,
    )
    .expect_err("the injected intent failure refuses the peer action");
    assert_eq!(err.code, ErrorCode::Internal);
    assert!(
        run.events_for(&peer_action_id(&request_id, b.device_id()))
            .is_empty(),
        "a failed intent leaves no event row"
    );
    assert_pending_row(run, &request_id, b);
    assert!(
        b.journal_facts().is_none(),
        "no delivery is attempted without the durable intent"
    );

    // The durable intent and delivery then run for real; the peer stays
    // unacknowledged.
    let proof_b = delivered(&deliver(run, b, &request_id)).clone();
    assert_eq!(proof_b.peer_id, b.device_id(), "b's own durable proof");
    mark_delivered(run.store(), &request_id, b.device_id()).expect("the delivery records");
    let proof_a = deliver_and_acknowledge(run, a, &request_id);
    assert_eq!(proof_a.peer_id, a.device_id(), "a's own durable proof");
    let action_a = peer_action_id(&request_id, a.device_id());

    // The outcome write fails over a real store that recorded the intent.
    let failed_outcome = FaultEvents::outcome_fails_over(run.store());
    let resume = resume_closure();
    let err = run
        .reconcile_with(&failed_outcome, &resume)
        .expect_err("the injected outcome failure refuses the settle");
    assert_eq!(err.code, ErrorCode::Internal);
    let rows = run.events_for(&action_a);
    assert_eq!(rows.len(), 1, "one event row for the peer action");
    assert_eq!(
        rows[0].state,
        LocalEventState::Pending,
        "a failed outcome leaves the action pending"
    );
    assert!(rows[0].result.is_none(), "no outcome is written");

    seed_subject_and_control(run, "s05");
    let erased = run.erase(&request_id).expect("the effect runs");
    assert_eq!(erased.status, "PROCESSING", "the peer work is still owed");
    let tombstones = run.tombstones(&request_id);

    // The restart reconciles the outcome exactly once.
    run.restart();
    let resume = resume_closure();
    let report = run
        .reconcile(&resume)
        .expect("the real pass settles the acknowledged peer");
    assert!(report.settled.contains(&action_a), "{report:?}");
    assert!(
        report
            .pending
            .contains(&peer_action_id(&request_id, b.device_id())),
        "the delivered peer stays pending: {report:?}"
    );
    let rows = run.events_for(&action_a);
    assert_eq!(rows.len(), 1, "exactly one durable result");
    assert_eq!(rows[0].state, LocalEventState::Allowed);
    let again = run.reconcile(&resume).expect("the second pass runs");
    assert!(
        !again.settled.contains(&action_a),
        "the outcome reconciles once"
    );

    // Pruning review events while the peer work is pending.
    let pruned = run
        .store()
        .prune_completed_before("2026-01-02T00:00:00Z")
        .expect("the prune runs");
    assert!(pruned >= 1, "the completed peer action is pruned");
    assert!(
        run.events_for(&action_a).is_empty(),
        "the completed action is gone"
    );
    let action_b = peer_action_id(&request_id, b.device_id());
    let rows = run.events_for(&action_b);
    assert_eq!(rows.len(), 1, "the pending peer action survives the prune");
    assert_eq!(rows[0].state, LocalEventState::Pending);
    assert_eq!(
        run.peer_row(&request_id, b.device_id()).state,
        PeerActionState::Delivered,
        "the durable denial state survives the prune"
    );
    assert!(
        b.journal_facts().is_some(),
        "the peer's recorded denial survives the prune"
    );
    let still = run
        .erase(&request_id)
        .expect("the pending peer still refuses");
    assert_eq!(still.status, "PROCESSING");
    assert_eq!(
        run.tombstones(&request_id),
        tombstones,
        "pruning repeats no purge"
    );
}

/// T06, disabled arm: a peers-disabled core records not-applicable and runs
/// the erasure without any peer process or peer row.
pub fn scenario_06_disabled<T: AcceptanceStore + 'static>(run: &mut Run<T>) {
    let request_id = open_request(run, "s06-disabled", &[]);
    let record = run.load_record(&request_id);
    assert_eq!(
        record.peer_enablement,
        PeerEnablement::NotApplicable,
        "a peers-disabled core deployment records a known fact"
    );
    assert!(record.peers.is_empty(), "no peer slot is frozen");
    assert!(
        run.peer_states(&request_id).is_empty(),
        "no peer state is derived"
    );

    seed_subject_and_control(run, "s06-disabled");
    let erased = run.erase(&request_id).expect("the core path runs");
    assert_eq!(
        erased.status, "COMPLETED",
        "peers-disabled is not a pending peer"
    );
    assert!(run.tombstones(&request_id) > 0, "the purge committed");
    assert!(
        run.store()
            .get_memory_item(BANK, "s06-disabled-control")
            .expect("the control is readable")
            .is_some(),
        "the control subject survives"
    );
    assert!(
        run.store()
            .load_for_request(&request_id)
            .expect("load the peer rows")
            .is_empty(),
        "no peer row exists without a required set"
    );
}

/// T06, invalid-result arm: a wrong peer, a wrong deployment, and an
/// unauthorized bearer each refuse without acknowledging or widening scope.
pub fn scenario_06_invalid<T: AcceptanceStore + 'static>(
    run: &mut Run<T>,
    a: &Node,
    b: &Node,
    c: &Node,
) {
    let request_id = open_request(run, "s06", &[a, b, c]);

    let wrong_peer = run
        .deliver_target(
            a.remote(),
            "wrong-peer",
            &request_id,
            SUBJECT,
            BANK,
            &[BANK],
            &run.fingerprint,
            Some(TOKEN),
        )
        .expect("the call is well formed");
    assert!(
        matches!(wrong_peer, DeliveryOutcome::Refused { .. }),
        "a proof from another peer cannot acknowledge: {wrong_peer:?}"
    );
    let wrong_deployment = run
        .deliver_target(
            b.remote(),
            b.device_id(),
            &request_id,
            SUBJECT,
            BANK,
            &[BANK],
            "another-deployment",
            Some(TOKEN),
        )
        .expect("the call is well formed");
    assert!(
        matches!(wrong_deployment, DeliveryOutcome::Refused { .. }),
        "a proof under another deployment cannot acknowledge: {wrong_deployment:?}"
    );
    let unauthorized = run
        .deliver_target(
            c.remote(),
            c.device_id(),
            &request_id,
            SUBJECT,
            BANK,
            &[BANK],
            &run.fingerprint,
            Some("not-the-harness-token"),
        )
        .expect("the call is well formed");
    assert!(
        matches!(unauthorized, DeliveryOutcome::Refused { .. }),
        "an unauthorized bearer cannot acknowledge: {unauthorized:?}"
    );

    // None of the invalid results acknowledges or widens the frozen scope.
    for node in [a, b, c] {
        assert_pending_row(run, &request_id, node);
    }
    let frozen = run.load_record(&request_id).peers;
    assert_eq!(frozen.len(), 3, "the frozen set is unchanged");
    let rows = run
        .store()
        .load_for_request(&request_id)
        .expect("load the peer rows");
    assert!(
        rows.iter().all(|row| row.state == PeerActionState::Pending),
        "no refusal records a delivery: {rows:?}"
    );

    seed_subject_and_control(run, "s06");
    let erased = run.erase(&request_id).expect("the effect runs");
    assert_eq!(
        erased.status, "PROCESSING",
        "no invalid result acknowledges, so completion refuses"
    );
    assert!(run.tombstones(&request_id) > 0, "the local purge committed");
    assert_eq!(
        run.load_record(&request_id).peers.len(),
        3,
        "the scope never widens"
    );
}
