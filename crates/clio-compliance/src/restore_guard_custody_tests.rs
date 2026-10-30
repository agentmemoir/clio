// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit coverage of the copy-custody gate
//! facts: the digest expression, the input building over a custody row, the
//! refusal set the gate returns, and the refusal mapping the guard and the
//! route share.
//!
//! ## Owns
//! - The absent-row, NULL-field, digest-mismatch, and receiver-mismatch
//!   mappings through `check_copy_custody` and `custody_refusal_parts`
//!
//! ## Does not own
//! - The guarded restore sequence (see `restore_guard_tests`)
//! - The gate's own rules (see `subject_request_events_gates_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the facts shape or the refusal mapping changes.
//!
//! Test-only. Never compiled into production builds.

use super::{
    CopyCustodyFacts, RestoreGuardRefusal, check_copy_custody, copy_digest_of,
    custody_refusal_parts,
};
use crate::subject_request_events::RestoreRefusal;
use clio_store::DestinationCustodyRow;
use clio_types::ErrorCode;

/// The digest a matching row records, in the delivery expression.
const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A custody row over `digest` and `receiver`, with both fields set.
fn row(digest: Option<&str>, receiver: Option<&str>) -> DestinationCustodyRow {
    DestinationCustodyRow {
        action_id: "act-copy".to_owned(),
        destination_path: "copy".to_owned(),
        custody_owner: "owner".to_owned(),
        required_action: "erase".to_owned(),
        state: "delivered".to_owned(),
        exception_reason: None,
        copy_digest: digest.map(str::to_owned),
        receiver: receiver.map(str::to_owned),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    }
}

/// The facts over `row`, checked by the restorer `system` against `DIGEST`.
fn facts(row: Option<DestinationCustodyRow>) -> CopyCustodyFacts<'static> {
    CopyCustodyFacts {
        row,
        copy_digest: DIGEST,
        restorer: "system",
    }
}

#[test]
fn an_absent_row_maps_to_the_manifest_missing_refusal() {
    let Err(refusal) = check_copy_custody(&facts(None)) else {
        panic!("an absent custody row must refuse");
    };
    assert_eq!(refusal, RestoreRefusal::CopyManifestMissing);
}

#[test]
fn a_row_without_a_recorded_digest_maps_to_the_manifest_missing_refusal() {
    let input = facts(Some(row(None, Some("system"))));
    let Err(refusal) = check_copy_custody(&input) else {
        panic!("a NULL digest must refuse");
    };
    assert_eq!(refusal, RestoreRefusal::CopyManifestMissing);
}

#[test]
fn a_row_with_a_different_digest_maps_to_the_digest_mismatch_refusal() {
    let input = facts(Some(row(
        Some("sha256:fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"),
        Some("system"),
    )));
    let Err(refusal) = check_copy_custody(&input) else {
        panic!("a digest mismatch must refuse");
    };
    assert_eq!(refusal, RestoreRefusal::CopyDigestMismatch);
}

#[test]
fn a_row_with_a_different_receiver_maps_to_the_receiver_mismatch_refusal() {
    let input = facts(Some(row(Some(DIGEST), Some("harness"))));
    let Err(refusal) = check_copy_custody(&input) else {
        panic!("a receiver mismatch must refuse");
    };
    assert_eq!(refusal, RestoreRefusal::ReceiverMismatch);
}

#[test]
fn a_matching_row_passes_the_gate() {
    let input = facts(Some(row(Some(DIGEST), Some("system"))));
    assert!(check_copy_custody(&input).is_ok());
}

#[test]
fn the_digest_expression_carries_the_algorithm_prefix() {
    assert_eq!(
        copy_digest_of(b"payload"),
        format!("sha256:{}", crate::bundle::sha256_hex_bytes(b"payload"))
    );
}

#[test]
fn the_refusal_mapping_keeps_the_gate_codes_and_text() {
    let (code, message) = custody_refusal_parts(RestoreRefusal::CopyManifestMissing);
    assert_eq!(code, ErrorCode::NotFound);
    assert!(message.contains("no copy-custody record"), "{message}");
    let (code, _) = custody_refusal_parts(RestoreRefusal::CopyDigestMismatch);
    assert_eq!(code, ErrorCode::Forbidden);
    let (code, _) = custody_refusal_parts(RestoreRefusal::ReceiverMismatch);
    assert_eq!(code, ErrorCode::Forbidden);
    let refusal = RestoreGuardRefusal::Custody(RestoreRefusal::CopyManifestMissing);
    assert_eq!(refusal.code(), ErrorCode::NotFound);
}
