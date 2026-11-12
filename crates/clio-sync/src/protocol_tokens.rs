// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the opaque sync tokens: the per-bank cursor
//! and the push-watermark token, their checksums, and the v1/v2 acceptance rule
//! for a token written by an older build.
//!
//! ## Owns
//! - Cursor encode/decode and watermark encode/decode
//!
//! ## Does not own
//! - The rest of the wire contract (`protocol`)
//!
//! ## Boundary
//!
//! Changes to a token's body or checksum should normally require changes here.

use base64::Engine;
use clio_types::{AmError, ErrorCode};

/// Decode an opaque cursor into its sequence number.
///
/// Cursors are opaque to clients; only the issuing peer decodes them.
///
/// # Errors
/// Malformed token or checksum mismatch.
pub fn decode_cursor(token: &str, bank: &str) -> Result<i64, AmError> {
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| AmError::new(ErrorCode::InvalidArgument, "malformed sync cursor"))?;
    let text = String::from_utf8(raw)
        .map_err(|_| AmError::new(ErrorCode::InvalidArgument, "malformed sync cursor"))?;
    let (seq, sum) = text
        .split_once(':')
        .ok_or_else(|| AmError::new(ErrorCode::InvalidArgument, "malformed sync cursor"))?;
    let seq: i64 = seq
        .parse()
        .map_err(|_| AmError::new(ErrorCode::InvalidArgument, "malformed sync cursor"))?;
    if !checksum_matches(bank, seq, sum) {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "sync cursor checksum mismatch",
        ));
    }
    Ok(seq)
}

/// Encode a sequence number as an opaque per-bank cursor token.
#[must_use]
pub fn encode_cursor(bank: &str, seq: i64) -> String {
    let text = format!("{seq}:{}", checksum(bank, seq));
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(text.as_bytes())
}

/// The current v2 checksum prefix for one bank and sequence number.
fn checksum(bank: &str, seq: i64) -> String {
    let preimage = format!("{bank}|{seq}");
    clio_store::domain_sha256_hex(clio_store::Domain::SyncCursor, &preimage)[..12].to_owned()
}

/// Whether `sum` is the current v2 checksum or the legacy v1 one.
///
/// A cursor token issued by an older build carries a v1 checksum, so decoding
/// must accept it. A malformed tag data file fails closed: the token is
/// refused rather than trusted.
fn checksum_matches(bank: &str, seq: i64, sum: &str) -> bool {
    if checksum(bank, seq) == sum {
        return true;
    }
    let preimage = format!("{bank}|{seq}");
    crate::legacy_tags::legacy_sha256_hex(crate::legacy_tags::SYNC_CURSOR, &preimage)
        .is_ok_and(|legacy| legacy[..12] == *sum)
}

/// Opaque watermark token codec for the client-side push change feed.
/// The watermark is a local timestamp, not a peer-issued cursor; clients own it.
pub fn encode_watermark(bank: &str, stamp: &str) -> String {
    let text = format!("wm:{stamp}:{}", checksum_str(bank, stamp));
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(text.as_bytes())
}

/// Decode an opaque watermark token into its timestamp.
///
/// # Errors
/// Malformed token or checksum mismatch.
pub fn decode_watermark(bank: &str, token: &str) -> Result<String, AmError> {
    if token.is_empty() {
        return Ok("1970-01-01T00:00:00Z".to_owned());
    }
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| AmError::new(ErrorCode::InvalidArgument, "malformed watermark"))?;
    let text = String::from_utf8(raw)
        .map_err(|_| AmError::new(ErrorCode::InvalidArgument, "malformed watermark"))?;
    let body = text
        .strip_prefix("wm:")
        .ok_or_else(|| AmError::new(ErrorCode::InvalidArgument, "malformed watermark"))?;
    let (stamp, sum) = body
        .rsplit_once(':')
        .ok_or_else(|| AmError::new(ErrorCode::InvalidArgument, "malformed watermark"))?;
    if !checksum_str_check(bank, stamp, sum) {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "watermark checksum mismatch",
        ));
    }
    Ok(stamp.to_owned())
}

/// The current v2 watermark checksum prefix for one bank and stamp.
fn checksum_str(bank: &str, stamp: &str) -> String {
    let preimage = format!("{bank}|{stamp}");
    clio_store::domain_sha256_hex(clio_store::Domain::SyncWatermark, &preimage)[..12].to_owned()
}

/// Whether `sum` is the current v2 watermark checksum or the legacy v1 one.
fn checksum_str_check(bank: &str, stamp: &str, sum: &str) -> bool {
    if stamp.is_empty() {
        return false;
    }
    if checksum_str(bank, stamp) == sum {
        return true;
    }
    let preimage = format!("{bank}|{stamp}");
    crate::legacy_tags::legacy_sha256_hex(crate::legacy_tags::SYNC_WATERMARK, &preimage)
        .is_ok_and(|legacy| legacy[..12] == *sum)
}

#[cfg(test)]
#[path = "protocol_tokens_tests.rs"]
mod protocol_tokens_tests;
