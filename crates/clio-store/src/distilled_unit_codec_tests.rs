// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the derivation record codec's contract tests:
//! the `source_ids_json` round trip and its rejection of malformed input, and
//! the stored-integer conversions with their out-of-range rejection.
//!
//! ## Owns
//! - The codec round-trip and error-path cases
//!
//! ## Does not own
//! - The record shape and claim-set rules (`distilled_unit_tests`)
//! - The backend row decoding that calls the codec (`sqlite_*` / `postgres_*`)
//!
//! ## Boundary
//!
//! Changes to the stored encoding or the accepted ranges should normally update
//! the cases here.
//!
//! Keep this module focused on the codec contract.
//!
//! Test-only. Never compiled into production builds.

use super::*;

#[test]
fn source_ids_round_trip_and_reject_malformed_json() {
    let ids = vec!["a".to_owned(), "b".to_owned()];
    let encoded = encode_source_ids(&ids);
    assert_eq!(decode_source_ids(&encoded).unwrap(), ids);
    assert!(
        decode_source_ids("not a json array").is_err(),
        "a malformed column is an error, not an empty list"
    );
}

#[test]
fn stored_integers_convert_and_reject_out_of_range() {
    assert_eq!(record_version_from_i64(3).unwrap(), 3);
    assert_eq!(revision_from_i64(1).unwrap(), 1);
    assert!(record_version_from_i64(-1).is_err());
    assert!(record_version_from_i64(i64::from(u32::MAX) + 1).is_err());
    assert!(revision_from_i64(-1).is_err());
    assert!(revision_from_i64(i64::from(u32::MAX) + 1).is_err());
}
