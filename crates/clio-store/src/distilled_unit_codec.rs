// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the derivation record's stored-value codec:
//! encoding and decoding the `source_ids_json` list and converting the stored
//! `record_version` and `revision` integers back into their in-memory types.
//!
//! ## Owns
//! - `encode_source_ids` / `decode_source_ids`
//! - `record_version_from_i64` / `revision_from_i64`
//!
//! ## Does not own
//! - The record shape, claim set, evidence rules, and validation
//!   (`distilled_unit`)
//! - The backend SQL that stores these columns (`sqlite_distilled_unit` /
//!   `postgres_distilled_unit`)
//!
//! ## Boundary
//!
//! Changes to the stored encoding or the accepted integer ranges should
//! normally require changes here. Changes to the record's own rules should not.
//!
//! Keep this module focused on stored-value conversion.

use crate::migrate::IntoAmError;
use clio_types::{AmError, ErrorCode};

/// Serialize a source-id list for the `source_ids_json` column.
pub(crate) fn encode_source_ids(ids: &[String]) -> String {
    serde_json::to_string(ids).expect("distilled unit source ids serialize")
}

/// Parse the `source_ids_json` column.
pub(crate) fn decode_source_ids(raw: &str) -> Result<Vec<String>, AmError> {
    serde_json::from_str(raw).am("distilled unit source ids")
}

/// Convert a stored `record_version` into the in-memory `u32`.
pub(crate) fn record_version_from_i64(raw: i64) -> Result<u32, AmError> {
    u32::try_from(raw).map_err(|_| {
        AmError::new(
            ErrorCode::Internal,
            format!("distilled unit record_version out of range: {raw}"),
        )
    })
}

/// Convert a stored `revision` into the in-memory `u32`. Pre-v2 rows store the
/// column default 1.
pub(crate) fn revision_from_i64(raw: i64) -> Result<u32, AmError> {
    u32::try_from(raw).map_err(|_| {
        AmError::new(
            ErrorCode::Internal,
            format!("distilled unit revision out of range: {raw}"),
        )
    })
}
