// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the copy-disposition wiring scenarios on
//! SQLite: every seeded copy carries its disposition metadata, an unknown
//! product path, a pending path, and an open caller action refuse completion
//! and closure, a gated retention exception survives a restart as a partial
//! outcome, and removal, retention, and delivery stay separate recorded
//! actions. The disposition parse and the privileged-actor gates have their
//! unit tests here too.
//!
//! ## Owns
//! - The copy-disposition scenarios on SQLite, the copy row builder,
//!   and the privileged recorder context
//! - The disposition parse and the gate refusal unit tests
//!
//! ## Does not own
//! - The wiring itself (`subject_request_store_wiring.rs`)
//! - The shared SQLite fixtures (`subject_request_store_wiring_tests.rs`)
//! - The live-Postgres scenario twin
//!   (`subject_request_store_wiring_copy_scenarios_pg_tests.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the row wiring stops attaching every copy row to a
//! request result or a named open gap, or when the gate stops refusing a
//! non-privileged recording.
//!
//! Test-only. Never compiled into production builds.

use super::subject_request_store_wiring_tests::{
    Scratch, kms, open_sqlite, persist, started_record, subject_scope,
};
use super::{
    closure_inputs, copy_row_closure_facts, copy_row_disposition, record_copy_disposition,
    record_retention_exception, reload_ledger,
};
use crate::bundle::sha256_hex_bytes;
use crate::copy_disposition::{CopyCustodyClass, CopyDispositionOutcome, CopyState};
use crate::subject_request::{
    PathOwner, PathSlot, PathState, RequestAction, RequestActionKind, RequestActionState,
};
use crate::subject_request_events::{ClosureBlock, accept_closure};
use crate::test_unique::unique;
use clio_store::{CopyInventoryRow, CopyInventoryStore, SqliteStore};
use clio_types::{Actor, Bank, ErrorCode, OpContext};

/// A system-actor context: the privileged recorder every scenario uses.
pub(super) fn ctx() -> OpContext {
    OpContext::new(Bank::new(unique("wiring-bank")), Actor::System)
}

/// A copy row with synthetic metadata values only: no subject content, no
/// file bytes, no tokens.
pub(super) fn copy_row(
    copy_id: &str,
    request_id: &str,
    custody: &str,
    action: &str,
    state: &str,
) -> CopyInventoryRow {
    CopyInventoryRow {
        copy_id: copy_id.to_owned(),
        request_id: request_id.to_owned(),
        custody: custody.to_owned(),
        owner: "system".to_owned(),
        required_action: action.to_owned(),
        state: state.to_owned(),
        exception_reason: None,
        updated_at: "2026-02-01T00:00:00Z".to_owned(),
    }
}

/// The reason every seeded copy names a controlled staging copy, a caller
/// bundle, a captured log, an archive, and a review output.
fn seeded_copies(request_id: &str) -> Vec<CopyInventoryRow> {
    vec![
        copy_row("copy-staging", request_id, "clio", "erase", "pending"),
        copy_row(
            "copy-caller-bundle",
            request_id,
            "caller",
            "remove",
            "pending",
        ),
        copy_row(
            "copy-captured-log",
            request_id,
            "operator",
            "remove",
            "pending",
        ),
        copy_row("copy-archive", request_id, "operator", "erase", "pending"),
        copy_row(
            "copy-review-output",
            request_id,
            "review_output",
            "erase",
            "pending",
        ),
    ]
}

/// Scenario 1 on SQLite: every seeded copy carries its custody, owner,
/// required action, and state after a restart, and the metadata holds only
/// the synthetic values.
#[test]
fn scenario_1_every_seeded_copy_carries_its_disposition_metadata_on_sqlite() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let request_id = unique("req");
    let rows = seeded_copies(&request_id);
    {
        let store = open_sqlite(&path);
        persist(&store, &started_record(&request_id, &subject_scope()));
        let recorder = ctx();
        for row in &rows {
            record_copy_disposition(&recorder, &store, row).expect("the disposition records");
        }
    }
    let store = open_sqlite(&path);
    let mut loaded = store.load_all().expect("the copy rows reload");
    loaded.sort_by(|a, b| a.copy_id.cmp(&b.copy_id));
    let mut expected = rows;
    expected.sort_by(|a, b| a.copy_id.cmp(&b.copy_id));
    assert_eq!(
        loaded, expected,
        "metadata rows carry only the synthetic values"
    );
}

/// Scenario 2 on SQLite: an unknown product path refuses technical
/// completion, an open caller action refuses closure with its matching
/// block, and the caller's files keep their hashes because the refusal
/// attempts no deletion.
#[test]
fn scenario_2_unknown_path_and_open_caller_action_refuse_completion_and_closure() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let request_id = unique("req");
    let caller_rows = vec![copy_row(
        "copy-caller-bundle",
        &request_id,
        "caller",
        "remove",
        "pending",
    )];
    let bundle = scratch.0.join("caller-bundle.bin");
    let log = scratch.0.join("caller-log.bin");
    std::fs::create_dir_all(&scratch.0).expect("create the scenario directory");
    std::fs::write(&bundle, b"caller bundle bytes").expect("write the caller bundle");
    std::fs::write(&log, b"caller log bytes").expect("write the caller log");
    let hashes_before = [
        sha256_hex_bytes(b"caller bundle bytes"),
        sha256_hex_bytes(b"caller log bytes"),
    ];
    {
        let store = open_sqlite(&path);
        persist(&store, &started_record(&request_id, &subject_scope()));
        let recorder = ctx();
        for row in &caller_rows {
            record_copy_disposition(&recorder, &store, row).expect("the disposition records");
        }
    }
    let store = open_sqlite(&path);
    let rows = store.load_all().expect("the copy rows reload");
    let ledger = reload_ledger(&store).expect("the ledger reloads");
    let record = ledger.record(&request_id).expect("the record reloads");
    let unknown = copy_row_closure_facts(record, &rows, "product-path").expect("the facts derive");
    assert!(
        matches!(
            accept_closure(&unknown.inputs),
            Err(ClosureBlock::UnknownProductPath)
        ),
        "an unknown product path must refuse technical completion"
    );
    let mut known_record = record.clone();
    known_record.paths.insert(
        "product-path".into(),
        PathSlot {
            owner: PathOwner::Clio,
            state: PathState::Done,
        },
    );
    let known =
        copy_row_closure_facts(&known_record, &rows, "product-path").expect("the facts derive");
    assert!(
        matches!(
            accept_closure(&known.inputs),
            Err(ClosureBlock::OpenCopyAction)
        ),
        "the open caller action must refuse closure with its matching block"
    );
    assert_ne!(
        known.outcome,
        CopyDispositionOutcome::Full,
        "an open caller action can never support a full claim"
    );
    let hashes_after = [
        sha256_hex_bytes(&std::fs::read(&bundle).expect("reread the caller bundle")),
        sha256_hex_bytes(&std::fs::read(&log).expect("reread the caller log")),
    ];
    assert_eq!(
        hashes_after, hashes_before,
        "the caller files keep their hashes"
    );
}

/// Scenario 3 on SQLite: a gated retention exception and its recording
/// owner survive a restart verbatim, and the outcome stays partial, never
/// full.
#[test]
fn scenario_3_a_recorded_retention_exception_survives_a_restart_as_partial() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let request_id = unique("req");
    let exception = "archive retained under the approved retention exception";
    let seeded = started_record(&request_id, &subject_scope());
    let archive = CopyInventoryRow {
        exception_reason: Some(exception.to_owned()),
        ..copy_row("copy-archive", &request_id, "operator", "erase", "done")
    };
    {
        let store = open_sqlite(&path);
        persist(&store, &seeded);
        record_copy_disposition(&ctx(), &store, &archive).expect("the disposition records");
        record_retention_exception(&ctx(), &store, &request_id, exception)
            .expect("the exception records through the gate");
        let restarted = reload_ledger(&store).expect("the ledger reloads");
        let before = restarted.record(&request_id).expect("the record reloads");
        assert_eq!(before.exception.as_deref(), Some(exception));
    }
    let store = open_sqlite(&path);
    let ledger = reload_ledger(&store).expect("the ledger reloads");
    let reloaded = ledger.record(&request_id).expect("the record reloads");
    assert_eq!(
        reloaded.exception.as_deref(),
        Some(exception),
        "the exception survives the restart verbatim"
    );
    let rows = store.load_all().expect("the copy rows reload");
    assert_eq!(rows, vec![archive], "the owner and reason survive verbatim");
    let facts = copy_row_closure_facts(reloaded, &rows, "product-path").expect("the facts derive");
    assert!(facts.inputs.retention_exception_approved);
    assert_eq!(
        facts.outcome,
        CopyDispositionOutcome::Partial,
        "an approved retention exception yields partial, never full"
    );
}

/// Scenario 4 on SQLite: a removed peer, a retained archive, and an
/// undelivered export stay three separate recorded actions, each with its
/// owner and durable proof; retention stays partial and the open delivery
/// prevents closure.
#[test]
fn scenario_4_removal_retention_and_delivery_stay_separate_actions() {
    let scratch = Scratch::new();
    let path = scratch.0.join("store.db");
    let request_id = unique("req");
    let exception = "archive retained under the approved retention exception";
    let mut seeded = started_record(&request_id, &subject_scope());
    seeded.paths.insert(
        "product-path".into(),
        PathSlot {
            owner: PathOwner::Clio,
            state: PathState::Done,
        },
    );
    let rows = vec![
        copy_row("copy-peer", &request_id, "peer", "remove", "done"),
        CopyInventoryRow {
            exception_reason: Some(exception.to_owned()),
            ..copy_row("copy-archive", &request_id, "operator", "erase", "done")
        },
        copy_row("copy-export", &request_id, "clio", "delivery", "pending"),
    ];
    {
        let store = open_sqlite(&path);
        persist(&store, &seeded);
        record_retention_exception(&ctx(), &store, &request_id, exception)
            .expect("the exception records through the gate");
        let recorder = ctx();
        for row in &rows {
            record_copy_disposition(&recorder, &store, row).expect("the disposition records");
        }
    }
    let store = open_sqlite(&path);
    for row in &rows {
        let proof = store
            .load(&row.copy_id)
            .expect("load")
            .expect("the stored row exists");
        assert_eq!(
            proof.owner, "system",
            "each action names its recording owner"
        );
        assert_eq!(&proof, row, "each action keeps its durable proof");
    }
    let ledger = reload_ledger(&store).expect("the ledger reloads");
    let reloaded = ledger.record(&request_id).expect("the record reloads");
    let stored = store.load_all().expect("the copy rows reload");
    let facts =
        copy_row_closure_facts(reloaded, &stored, "product-path").expect("the facts derive");
    assert_eq!(
        facts.outcome,
        CopyDispositionOutcome::Partial,
        "retention stays partial"
    );
    assert!(
        matches!(
            accept_closure(&facts.inputs),
            Err(ClosureBlock::OpenCopyAction)
        ),
        "the open delivery prevents closure"
    );
}

/// A recorded pending or failed product path refuses completion and closure.
#[test]
fn a_recorded_pending_or_failed_path_refuses_completion() {
    let mut record = started_record(&unique("req"), &subject_scope());
    for state in [PathState::Pending, PathState::Failed] {
        record.paths.insert(
            "product-path".into(),
            PathSlot {
                owner: PathOwner::Clio,
                state: state.clone(),
            },
        );
        let inputs = closure_inputs(&record, "product-path");
        assert!(
            matches!(
                accept_closure(&inputs),
                Err(ClosureBlock::UnknownProductPath)
            ),
            "a {state:?} path must refuse completion"
        );
    }
}

/// An open record-level notice or delivery action blocks closure.
#[test]
fn open_notice_and_delivery_actions_block_closure() {
    for kind in [RequestActionKind::Notice, RequestActionKind::Delivery] {
        let mut record = started_record(&unique("req"), &subject_scope());
        record.paths.insert(
            "product-path".into(),
            PathSlot {
                owner: PathOwner::Clio,
                state: PathState::Done,
            },
        );
        record.actions.insert(
            "action-1".into(),
            RequestAction {
                kind,
                state: RequestActionState::Open,
            },
        );
        let inputs = closure_inputs(&record, "product-path");
        assert!(
            matches!(accept_closure(&inputs), Err(ClosureBlock::OpenCopyAction)),
            "{kind:?} must block closure"
        );
    }
}

/// A disposition recorded by a non-privileged actor is refused.
#[test]
fn a_disposition_from_a_non_privileged_actor_is_refused() {
    let store = SqliteStore::open_in_memory(kms()).expect("in-memory sqlite opens");
    let row = copy_row("copy-1", "req-1", "clio", "erase", "pending");
    let user = OpContext::new(Bank::new("wiring-bank"), Actor::Agent);
    let err = record_copy_disposition(&user, &store, &row)
        .expect_err("a non-privileged actor must refuse");
    assert_eq!(err.code, ErrorCode::Forbidden);
}

/// A disposition's owner must name the actor that recorded it.
#[test]
fn a_disposition_owner_must_name_the_recording_actor() {
    let store = SqliteStore::open_in_memory(kms()).expect("in-memory sqlite opens");
    let row = copy_row("copy-1", "req-1", "clio", "erase", "pending");
    let harness = OpContext::new(Bank::new("wiring-bank"), Actor::Harness);
    let err = record_copy_disposition(&harness, &store, &row)
        .expect_err("a row naming another actor must refuse");
    assert_eq!(err.code, ErrorCode::Forbidden);
}

/// The exception gate refuses an unprivileged actor, a blank reason, and an
/// unknown request.
#[test]
fn the_retention_exception_gate_refuses_invalid_inputs() {
    let store = SqliteStore::open_in_memory(kms()).expect("in-memory sqlite opens");
    let user = OpContext::new(Bank::new("wiring-bank"), Actor::Agent);
    let refused = record_retention_exception(&user, &store, "req-1", "retained")
        .expect_err("a non-privileged actor must refuse");
    assert_eq!(refused.code, ErrorCode::Forbidden);
    let blank = record_retention_exception(&ctx(), &store, "req-1", "  ")
        .expect_err("a blank reason must refuse");
    assert_eq!(blank.code, ErrorCode::InvalidArgument);
    let missing = record_retention_exception(&ctx(), &store, "req-1", "retained")
        .expect_err("an unknown request must refuse");
    assert_eq!(missing.code, ErrorCode::NotFound);
}

/// An owner or a required action the vocabulary does not hold refuses the
/// row instead of guessing.
#[test]
fn an_unparsed_owner_or_action_refuses_the_row() {
    let action = copy_row("copy-1", "req-1", "clio", "confirm_deletion", "pending");
    assert_eq!(
        copy_row_disposition(&action).unwrap_err().code,
        ErrorCode::Internal,
        "an unrecognized required action must refuse"
    );
    let mut owner = copy_row("copy-1", "req-1", "clio", "erase", "pending");
    owner.owner = "clio".to_owned();
    assert_eq!(
        copy_row_disposition(&owner).unwrap_err().code,
        ErrorCode::Internal,
        "an unrecognized owner must refuse"
    );
}

/// Custody or state the vocabulary does not hold fails closed to its own
/// unknown variant, which refuses any completion claim.
#[test]
fn an_unparsed_custody_or_state_fails_closed_to_unknown() {
    let request_id = unique("req");
    let row = copy_row("copy-1", &request_id, "subject_local", "erase", "open_gap");
    let parsed = copy_row_disposition(&row).expect("unknown custody and state parse fail-closed");
    assert_eq!(parsed.custody, CopyCustodyClass::Unknown);
    assert_eq!(parsed.state, CopyState::Unknown);
    let record = started_record(&request_id, &subject_scope());
    let facts = copy_row_closure_facts(&record, &[row], "product-path").expect("the facts derive");
    assert_eq!(
        facts.outcome,
        CopyDispositionOutcome::Refused,
        "unknown custody must refuse any completion claim"
    );
}
