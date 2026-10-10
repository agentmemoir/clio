// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for every rule that decides whether a token
//! record is valid: the per-field validators the write path calls on caller
//! input, the whole-store validator the load path calls on a file it did not
//! write, and the constant-time comparison a transport uses to check a
//! presented secret against a stored digest.
//!
//! ## Owns
//! - Label, bank-set, and `created_at` validation, shared by the writer and
//!   the loader so the two halves of one store cannot disagree
//! - `validate_loaded`: the whole-store pass (every record, then repeated
//!   labels) that turns a corrupt file into a refusal
//! - `ct_eq`, the constant-time byte comparison promoted from the MCP HTTP
//!   transport
//!
//! ## Does not own
//! - The record shape and its serialization (see `auth_tokens`)
//! - Reading, writing, locking, or permission-checking the store file
//! - The store-file path resolution
//!
//! ## Boundary
//!
//! Changes to what counts as a valid record should normally require changes
//! here. Changes to where the record is stored or who reads it should not.
//!
//! Keep this module focused on the validity rules and nothing else.
//!
//! # One validator, two callers
//!
//! The write path validates caller input and the load path validates a file
//! that may have been hand-edited, so both call the same functions here. A
//! loader that only checked "the banks list is not empty" would accept a
//! record holding an empty or repeated bank name that the writer would have
//! refused, and every later phase that inherits this store would inherit that
//! disagreement.

use crate::auth_tokens::AuthTokenRecord;
use crate::context::Actor;
use crate::error::{AmError, ErrorCode};
use clio_types::{civil_from_days, days_from_civil};

/// Label length cap; a store of a few dozen rows never needs a longer label.
const MAX_LABEL_LEN: usize = 64;

/// A label is 1..=64 characters of ASCII letters, digits, `_`, `-`, or `.`.
///
/// # Errors
/// [`ErrorCode::InvalidArgument`] with the rule in the message.
pub fn validate_label(label: &str) -> Result<(), AmError> {
    let ok = !label.is_empty()
        && label.len() <= MAX_LABEL_LEN
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if ok {
        return Ok(());
    }
    Err(AmError::new(
        ErrorCode::InvalidArgument,
        "label must be 1..=64 characters of letters, digits, `_`, `-`, or `.`",
    ))
}

/// At least one bank; every bank non-empty; duplicates collapse in first-seen
/// order.
///
/// # Errors
/// [`ErrorCode::InvalidArgument`] when no bank remains.
pub fn validate_banks(banks_raw: &[String]) -> Result<Vec<String>, AmError> {
    let mut banks: Vec<String> = Vec::new();
    for raw in banks_raw {
        let bank = raw.trim();
        if !bank.is_empty() && !banks.iter().any(|b| b == bank) {
            banks.push(bank.to_owned());
        }
    }
    if banks.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "a token must name at least one bank",
        ));
    }
    Ok(banks)
}

/// Constant-time byte equality (a length difference returns early).
///
/// Promoted from the MCP HTTP transport so both transports verify presented
/// tokens with the one shared implementation. The early return is acceptable
/// for a fixed-length bearer token, where the length is already known from
/// the request; it is not acceptable for a variable-length secret, where it
/// leaks the length, so such a secret must be compared in fixed-length form
/// (the transports compare digests, which are).
#[must_use]
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Validate every loaded record and refuse a repeated label.
///
/// A loaded record runs through the same validators the write path uses —
/// [`validate_label`], [`Actor::parse`], [`validate_banks`] — so a hand-edited
/// store cannot hand a later transport a record the writer would have refused.
/// The one deliberate difference is `created_at`: the loader checks its shape
/// rather than re-stamping it, so free text never reaches a surface that shows
/// it. Banks are compared after [`validate_banks`]' trimming, so `" demo "` and
/// `"demo"` count as the repeated entry they are.
///
/// # Errors
/// [`ErrorCode::ConfigCorrupt`] naming the offending record.
pub fn validate_loaded(records: &[AuthTokenRecord]) -> Result<(), AmError> {
    for (index, record) in records.iter().enumerate() {
        let named = || format!("token store record {index} (label `{}`)", record.label);
        let as_corrupt = |error: AmError| AmError::new(ErrorCode::ConfigCorrupt, error.message);
        validate_label(&record.label).map_err(|e| {
            AmError::new(
                ErrorCode::ConfigCorrupt,
                format!("{}: {}", named(), e.message),
            )
        })?;
        Actor::parse(record.actor.as_str()).map_err(as_corrupt)?;
        let banks = validate_banks(&record.banks).map_err(|e| {
            AmError::new(
                ErrorCode::ConfigCorrupt,
                format!("{}: {}", named(), e.message),
            )
        })?;
        if banks.len() != record.banks.len() {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                format!(
                    "{}: banks repeats or holds an empty entry: {}",
                    named(),
                    banks.join(", ")
                ),
            ));
        }
        if record.secret_sha256.len() != 64
            || !record
                .secret_sha256
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                format!("{}: secret_sha256 must be 64 lowercase hex digits", named()),
            ));
        }
        if !valid_created_at(&record.created_at) {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                format!(
                    "{}: created_at must be a UTC RFC 3339 stamp like 2026-10-01T00:00:00Z",
                    named()
                ),
            ));
        }
    }
    for (index, record) in records.iter().enumerate() {
        if records[..index].iter().any(|r| r.label == record.label) {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                format!("token store has a repeated label `{}`", record.label),
            ));
        }
    }
    Ok(())
}

/// Whether a `created_at` is a real UTC instant in the shape [`utc_stamp`]
/// writes: `YYYY-MM-DDThh:mm:ssZ`.
///
/// The round trip through [`clio_types::days_from_civil`] is what makes this
/// a date check rather than a punctuation check: it rejects a day the calendar
/// does not have (`2024-02-30`) as well as a free-text or out-of-range value. A
/// shape check, not a clock comparison, so it holds on any machine.
#[must_use]
pub fn valid_created_at(stamp: &str) -> bool {
    let bytes = stamp.as_bytes();
    if bytes.len() != 20 || bytes[19] != b'Z' {
        return false;
    }
    let digits = |mut range: std::ops::Range<usize>| range.all(|i| bytes[i].is_ascii_digit());
    if !(digits(0..4) && bytes[4] == b'-' && digits(5..7) && bytes[7] == b'-' && digits(8..10))
        || bytes[10] != b'T'
        || !digits(11..13)
        || bytes[13] != b':'
        || !digits(14..16)
        || bytes[16] != b':'
        || !digits(17..19)
    {
        return false;
    }
    let at = |range: std::ops::Range<usize>| {
        range
            .map(|i| u32::from(bytes[i] - b'0'))
            .fold(0u32, |acc, d| acc * 10 + d)
    };
    let (year, month, day, hour, minute, second) = (
        i64::from(at(0..4)),
        at(5..7),
        at(8..10),
        at(11..13),
        at(14..16),
        at(17..19),
    );
    if !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 59 {
        return false;
    }
    let days = days_from_civil(year, month, day);
    let back = civil_from_days(days);
    back == (year, month, day)
}

#[cfg(test)]
#[path = "auth_tokens_validation_tests.rs"]
mod auth_tokens_validation_tests;
