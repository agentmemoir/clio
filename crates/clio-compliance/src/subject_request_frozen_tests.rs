// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit coverage of the durable subject
//! request record: canonical serialization and verbatim reload.
//!
//! ## Owns
//! - Round-trip and stable-spelling assertions for the frozen vocabulary
//! - Restart assertions that path and operator slots reload verbatim
//!
//! ## Does not own
//! - Register binding assertions (`subject_request_register_tests.rs`)
//! - Any store, fixture, or I/O: every test here is pure
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::subject_request::{
    PathOwner, PathSlot, PathState, PeerEnablement, PeerState, RequestAction, RequestActionKind,
    RequestActionState, RequestKind, RequestScope, RequestState, SubjectRequestLedger,
    SubjectRequestRecord, VerificationResult,
};
use std::collections::{BTreeMap, BTreeSet};

fn sample_record() -> SubjectRequestRecord {
    let mut paths = BTreeMap::new();
    paths.insert(
        "archive-copy".to_owned(),
        PathSlot {
            owner: PathOwner::External,
            state: PathState::Pending,
        },
    );
    paths.insert(
        "active-dek".to_owned(),
        PathSlot {
            owner: PathOwner::Clio,
            state: PathState::Saved {
                attempt_id: "attempt-7".to_owned(),
                action_id: "action-9".to_owned(),
            },
        },
    );
    paths.insert(
        "derived-index".to_owned(),
        PathSlot {
            owner: PathOwner::Clio,
            state: PathState::Done,
        },
    );
    paths.insert(
        "purge-failed".to_owned(),
        PathSlot {
            owner: PathOwner::Clio,
            state: PathState::Failed,
        },
    );
    let mut actions = BTreeMap::new();
    actions.insert(
        "copy-to-subject".to_owned(),
        RequestAction {
            kind: RequestActionKind::Copy,
            state: RequestActionState::Open,
        },
    );
    let mut peers = BTreeMap::new();
    peers.insert("peer-1".to_owned(), PeerState::Pending);
    peers.insert("peer-2".to_owned(), PeerState::Acknowledged);
    peers.insert("peer-3".to_owned(), PeerState::Disposed);
    SubjectRequestRecord {
        request_id: "r1".to_owned(),
        kind: RequestKind::Erasure,
        scope: RequestScope::Subject {
            bank_id: "bank-a".to_owned(),
            subject_id: "subject-1".to_owned(),
        },
        state: RequestState::TechnicalComplete,
        verification: Some(VerificationResult::Verified),
        deadline: Some("2026-11-01".to_owned()),
        actions,
        paths,
        peers,
        peer_enablement: PeerEnablement::Required,
        peer_banks: BTreeSet::from(["bank-b".to_owned()]),
        exception: Some("approved archival hold".to_owned()),
    }
}

#[test]
fn a_fully_populated_record_round_trips_through_one_canonical_value() {
    let record = sample_record();
    let text = serde_json::to_string(&record).expect("serialize record");
    let reloaded: SubjectRequestRecord = serde_json::from_str(&text).expect("deserialize record");
    assert_eq!(reloaded, record);
    let again = serde_json::to_string(&reloaded).expect("re-serialize record");
    assert_eq!(again, text, "the value form must be canonical");
}

#[test]
fn the_frozen_spellings_are_stable_across_a_restart() {
    let spellings = [
        (
            serde_json::to_string(&RequestKind::Copy).unwrap(),
            "\"copy\"",
        ),
        (
            serde_json::to_string(&RequestKind::Erasure).unwrap(),
            "\"erasure\"",
        ),
        (
            serde_json::to_string(&VerificationResult::Unverified).unwrap(),
            "\"unverified\"",
        ),
        (
            serde_json::to_string(&RequestActionKind::Delivery).unwrap(),
            "\"delivery\"",
        ),
        (
            serde_json::to_string(&RequestActionKind::Notice).unwrap(),
            "\"notice\"",
        ),
        (
            serde_json::to_string(&RequestActionState::Open).unwrap(),
            "\"open\"",
        ),
        (
            serde_json::to_string(&RequestActionState::Closed).unwrap(),
            "\"closed\"",
        ),
        (
            serde_json::to_string(&PathOwner::External).unwrap(),
            "\"external\"",
        ),
        (
            serde_json::to_string(&PeerState::Acknowledged).unwrap(),
            "\"acknowledged\"",
        ),
        (
            serde_json::to_string(&PeerEnablement::NotApplicable).unwrap(),
            "\"not_applicable\"",
        ),
        (
            serde_json::to_string(&PeerEnablement::Required).unwrap(),
            "\"required\"",
        ),
    ];
    for (spelled, expected) in spellings {
        assert_eq!(spelled, expected);
    }
}

#[test]
fn a_restart_reloads_every_path_verbatim_without_auto_completing() {
    let ledger = SubjectRequestLedger::load(vec![sample_record()]);
    let record = ledger.record("r1").expect("record reloaded");
    assert_eq!(record.paths.len(), 4);
    assert_eq!(
        record.paths["active-dek"].state,
        PathState::Saved {
            attempt_id: "attempt-7".to_owned(),
            action_id: "action-9".to_owned(),
        },
        "the saved attempt and action ids must survive the restart"
    );
    assert_eq!(record.paths["archive-copy"].state, PathState::Pending);
    assert_eq!(record.paths["purge-failed"].state, PathState::Failed);
    assert_eq!(record.paths["derived-index"].state, PathState::Done);
    assert_eq!(record.paths["archive-copy"].owner, PathOwner::External);
}

#[test]
fn the_peer_enablement_fact_loads_from_an_older_row_and_fails_closed_on_an_unknown_spelling() {
    let parsed: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&sample_record()).expect("serialize record"))
            .expect("parse record");
    assert_eq!(parsed["peer_enablement"], serde_json::json!("required"));
    let mut older = parsed.clone();
    older
        .as_object_mut()
        .expect("the record is an object")
        .remove("peer_enablement");
    let legacy: SubjectRequestRecord = serde_json::from_value(older).expect("an older row loads");
    assert_eq!(
        legacy.peer_enablement,
        PeerEnablement::NotApplicable,
        "a row written before the fact loads as the known not-applicable value"
    );
    let mut damaged = parsed;
    damaged["peer_enablement"] = serde_json::json!("mystery");
    assert!(
        serde_json::from_value::<SubjectRequestRecord>(damaged).is_err(),
        "an unrecognized spelling must fail closed"
    );
}

/// A row written before the frozen peer-bank set carries no value and loads
/// as an empty set, so the pre-effect check covers the scope bank only.
#[test]
fn a_row_without_the_peer_bank_set_loads_an_empty_set() {
    let parsed: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&sample_record()).expect("serialize record"))
            .expect("parse record");
    assert_eq!(parsed["peer_banks"], serde_json::json!(["bank-b"]));
    let mut older = parsed;
    older
        .as_object_mut()
        .expect("the record is an object")
        .remove("peer_banks");
    let legacy: SubjectRequestRecord = serde_json::from_value(older).expect("an older row loads");
    assert!(
        legacy.peer_banks.is_empty(),
        "a row from before the fact loads an empty set"
    );
}

#[test]
fn a_restart_reloads_the_operator_fields_verbatim() {
    let ledger = SubjectRequestLedger::load(vec![sample_record()]);
    let record = ledger.record("r1").expect("record reloaded");
    assert_eq!(record.verification, Some(VerificationResult::Verified));
    assert_eq!(record.deadline.as_deref(), Some("2026-11-01"));
    assert_eq!(record.exception.as_deref(), Some("approved archival hold"));
    assert_eq!(
        record.actions["copy-to-subject"].state,
        RequestActionState::Open
    );
    assert_eq!(record.peers.get("peer-1"), Some(&PeerState::Pending));
    assert_eq!(record.state, RequestState::TechnicalComplete);
}
