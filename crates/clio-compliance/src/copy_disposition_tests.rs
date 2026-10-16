// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the copy-disposition vocabulary: which custody classes,
//! retention exceptions, and closure facts let a copy claim full completion,
//! when unknown or failed states refuse it, how request-level flags pass
//! through the fold, and that serde variants fail closed and roundtrip.
//!
//! ## Owns
//! - The disposition-claim and closure-fold assertions
//! - The resolved-copy and open-action helper fixtures
//! - The peer-copy mapping fold assertions
//! - The JSON fail-closed and roundtrip assertions
//!
//! ## Does not own
//! - The copy-disposition vocabulary and fold implementation (`copy_disposition`)
//! - The SQLite and Postgres store assertions (`copy_inventory_tests`,
//!   `copy_inventory_pg_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the disposition vocabulary, the closure-fact
//! rules, or the serde behavior changes. Store persistence belongs to its
//! own suites.
//!
//! Test-only. Never compiled into production builds.

use super::*;
use clio_store::PeerActionDisposition;

/// A fully resolved copy under known custody with every action closed.
fn resolved(copy_id: &str, custody: CopyCustodyClass) -> CopyDisposition {
    CopyDisposition {
        copy_id: copy_id.to_string(),
        custody,
        owner: CopyOwner::System,
        state: CopyState::Done,
        actions: Vec::new(),
    }
}

/// A copy whose technical erasure is done but whose notice is still owed.
fn done_with_open_action(kind: CopyRequiredAction) -> CopyDisposition {
    CopyDisposition {
        copy_id: "copy-notice-open".to_string(),
        custody: CopyCustodyClass::Clio,
        owner: CopyOwner::Harness,
        state: CopyState::Done,
        actions: vec![CopyAction {
            kind,
            state: RequestActionState::Open,
        }],
    }
}

#[test]
fn resolved_copies_under_known_custody_allow_full_claim() {
    let copies = [
        resolved("clio-copy", CopyCustodyClass::Clio),
        resolved("peer-copy", CopyCustodyClass::Peer),
        resolved("review-copy", CopyCustodyClass::ReviewOutput),
    ];
    let facts = copy_closure_facts(&copies, true, false);
    assert_eq!(facts.inputs.open_copy_actions, 0);
    assert_eq!(facts.outcome, CopyDispositionOutcome::Full);
}

#[test]
fn unknown_custody_refuses_any_completion_claim() {
    let mut unknown = resolved("copy-unknown", CopyCustodyClass::Unknown);
    unknown.state = CopyState::Done;
    let facts = copy_closure_facts(&[unknown], true, false);
    assert_eq!(facts.inputs.open_copy_actions, 1);
    assert_eq!(facts.outcome, CopyDispositionOutcome::Refused);
}

#[test]
fn approved_retention_exception_yields_partial_never_full() {
    let copies = [resolved("clio-copy", CopyCustodyClass::Clio)];
    let facts = copy_closure_facts(&copies, true, true);
    assert_eq!(facts.inputs.open_copy_actions, 0);
    assert!(facts.inputs.retention_exception_approved);
    assert_eq!(facts.outcome, CopyDispositionOutcome::Partial);
}

#[test]
fn open_notice_keeps_a_technically_erased_copy_open() {
    let copies = [done_with_open_action(CopyRequiredAction::Notice)];
    let facts = copy_closure_facts(&copies, true, false);
    assert_eq!(facts.inputs.open_copy_actions, 1);
    assert_eq!(facts.outcome, CopyDispositionOutcome::Partial);
}

#[test]
fn open_delivery_keeps_a_technically_erased_copy_open() {
    let copies = [done_with_open_action(CopyRequiredAction::Delivery)];
    let facts = copy_closure_facts(&copies, true, false);
    assert_eq!(facts.inputs.open_copy_actions, 1);
    assert_eq!(facts.outcome, CopyDispositionOutcome::Partial);
}

#[test]
fn closed_peer_removal_does_not_acknowledge_delivery() {
    let copy = CopyDisposition {
        copy_id: "peer-copy".to_string(),
        custody: CopyCustodyClass::Peer,
        owner: CopyOwner::System,
        state: CopyState::Done,
        actions: vec![
            CopyAction {
                kind: CopyRequiredAction::Remove,
                state: RequestActionState::Closed,
            },
            CopyAction {
                kind: CopyRequiredAction::Delivery,
                state: RequestActionState::Open,
            },
        ],
    };
    let facts = copy_closure_facts(&[copy], true, false);
    assert!(facts.inputs.open_copy_actions > 0);
    assert!(matches!(facts.outcome, CopyDispositionOutcome::Partial));
}

#[test]
fn pending_failed_and_unknown_states_refuse_technical_completion() {
    for state in [CopyState::Pending, CopyState::Failed, CopyState::Unknown] {
        let copy = CopyDisposition {
            copy_id: "copy-open".to_string(),
            custody: CopyCustodyClass::Clio,
            owner: CopyOwner::Harness,
            state,
            actions: Vec::new(),
        };
        let facts = copy_closure_facts(&[copy], true, false);
        assert_eq!(facts.inputs.open_copy_actions, 1, "state {state:?}");
        assert_eq!(facts.outcome, CopyDispositionOutcome::Partial);
    }
}

#[test]
fn facts_pass_request_level_flags_through() {
    let facts = copy_closure_facts(&[], false, false);
    assert!(!facts.inputs.product_path_known);
    assert_eq!(facts.inputs.open_copy_actions, 0);
    assert!(!facts.inputs.retention_exception_approved);
    assert_eq!(facts.outcome, CopyDispositionOutcome::Full);
}

#[test]
fn unknown_serde_variants_fail_closed() {
    let bad_custody = r#"{"copy_id":"c","custody":"somewhere_else","owner":"system","state":"done","actions":[]}"#;
    let err = serde_json::from_str::<CopyDisposition>(bad_custody).unwrap_err();
    assert_ne!(err.to_string(), "");
    let bad_state =
        r#"{"copy_id":"c","custody":"clio","owner":"system","state":"half_done","actions":[]}"#;
    assert!(serde_json::from_str::<CopyDisposition>(bad_state).is_err());
}

#[test]
fn disposition_roundtrips_through_json() {
    let copy = CopyDisposition {
        copy_id: "peer-copy".to_string(),
        custody: CopyCustodyClass::Peer,
        owner: CopyOwner::Harness,
        state: CopyState::Pending,
        actions: vec![CopyAction {
            kind: CopyRequiredAction::Remove,
            state: RequestActionState::Open,
        }],
    };
    let encoded = serde_json::to_string(&copy).unwrap();
    assert_eq!(
        serde_json::from_str::<CopyDisposition>(&encoded).unwrap(),
        copy
    );
}

/// A removal-evidence peer copy is a resolved copy: the technical state is
/// done, the removal action is closed, and the fold claims full.
#[test]
fn a_removal_evidence_peer_copy_folds_to_full() {
    let copy = peer_copy_disposition(
        "peer-a",
        PeerActionDisposition::RemovalEvidence,
        Some("deletion-9"),
    );
    assert_eq!(copy.custody, CopyCustodyClass::Peer, "the peer holds it");
    assert_eq!(copy.copy_id, "peer:peer-a", "the peer's derived copy id");
    assert_eq!(copy.state, CopyState::Done, "the removal is proven");
    assert_eq!(copy.actions[0].kind, CopyRequiredAction::Remove);
    assert_eq!(copy.actions[0].state, RequestActionState::Closed);
    let facts = copy_closure_facts(&[copy], true, false);
    assert_eq!(facts.inputs.open_copy_actions, 0);
    assert_eq!(facts.outcome, CopyDispositionOutcome::Full);
}

/// An approved retention peer copy keeps an open removal action: the fold is
/// partial, never a full completion claim.
#[test]
fn an_approved_retention_peer_copy_folds_to_partial() {
    let copy = peer_copy_disposition(
        "peer-a",
        PeerActionDisposition::RetentionApproved,
        Some("retention-7"),
    );
    assert_eq!(copy.custody, CopyCustodyClass::Peer, "the peer holds it");
    assert_eq!(
        copy.state,
        CopyState::Pending,
        "the retention is not removal"
    );
    assert_eq!(copy.actions[0].kind, CopyRequiredAction::Remove);
    assert_eq!(copy.actions[0].state, RequestActionState::Open);
    let facts = copy_closure_facts(&[copy], true, false);
    assert_eq!(facts.inputs.open_copy_actions, 1);
    assert_eq!(facts.outcome, CopyDispositionOutcome::Partial);
}

/// An unknown peer disposition keeps the copy's technical state unknown and
/// its removal action open, folding to partial while the custody stays known.
#[test]
fn an_unknown_peer_disposition_folds_to_partial_with_an_open_action() {
    let copy = peer_copy_disposition("peer-a", PeerActionDisposition::Unknown, None);
    assert_eq!(copy.custody, CopyCustodyClass::Peer, "the peer holds it");
    assert_eq!(
        copy.state,
        CopyState::Unknown,
        "no technical result is known"
    );
    assert_eq!(copy.actions[0].kind, CopyRequiredAction::Remove);
    assert_eq!(copy.actions[0].state, RequestActionState::Open);
    let facts = copy_closure_facts(&[copy], true, false);
    assert_eq!(facts.inputs.open_copy_actions, 1);
    assert_eq!(facts.outcome, CopyDispositionOutcome::Partial);
}
