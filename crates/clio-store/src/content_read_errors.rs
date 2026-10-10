// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one decision: does a stored-content read
//! failure mean *this row is unavailable*, or does it mean *the backend is
//! broken*.
//!
//! ## Owns
//! - `is_unreadable_content`, the single classification every optional-row reader uses
//! - The documented `Internal` exclusion that keeps backend failures fail-closed
//! - The novelty consequence of skipping a row, stated in one place
//!
//! ## Does not own
//! - Which code a content read produces (see `dek`, `item_persist`, `content_envelope`)
//! - Whether a given caller treats a row as optional (see `clio-mcp`, `clio-retrieve`)
//! - Admission scoring and duplicate detection
//!
//! ## Boundary
//!
//! Changes to how content-read failures are classified should normally require
//! changes here. Changes to which operations fail should not require changes
//! here.
//!
//! Keep this module focused on the skippable-versus-fail-closed decision.
//!
//! # Why this lives in the store, not on the error type
//!
//! "Forbidden means this row's content is unavailable" is a fact about stored
//! content. It is true because the only producers of `Forbidden` on a
//! single-row content read are the DEK paths: no DEK, a DEK version mismatch, or
//! an AEAD authentication failure. It is *not* true of `Forbidden` in general —
//! the attribute read paths also produce it, and there it means the caller is not
//! allowed to see the row. Putting this predicate on `AmError` would have
//! published a storage-recovery rule as a property of every error in the
//! workspace, for callers that have no business skipping authorization failures.

use clio_types::{AmError, ErrorCode};

/// True when `error` means *this one row's* stored content could not be opened,
/// so a reader that treats the row as optional may skip it and carry on.
///
/// Covers every way a row's content becomes unavailable:
///
/// - [`ErrorCode::UnreadableContent`] — malformed envelope, undecodable nonce or
///   ciphertext, missing content column, or sealed bytes that are not this
///   build's content format.
/// - [`ErrorCode::Forbidden`] — no DEK for the subject, a DEK version mismatch,
///   or an AEAD authentication failure.
/// - [`ErrorCode::ErasedSubject`] — the subject's DEK was destroyed
///   (crypto-shred); the payload is permanently unrecoverable.
///
/// # Why `Internal` is excluded
///
/// [`ErrorCode::Internal`] is also what a backend failure looks like: a driver
/// error, an unreachable database, an unreadable key-store file. A caller that
/// skipped on `Internal` would admit writes and serve reads on guessed signals
/// while the store was actually broken, which is exactly what
/// `factor_unavailable` exists to prevent. So backend failures stay
/// fail-closed, and that is the whole reason this is a named predicate instead of
/// a skip list at each call site: one definition cannot drift from the argument.
///
/// # What skipping costs
///
/// A skipped row contributes nothing to the signals computed from its
/// neighbours. For the admission novelty factor that means the candidate looks
/// *less* like a duplicate, so a near-duplicate of a damaged row can pass the
/// duplicate gate. That is a deliberate trade: one damaged row must not make the
/// bank unwritable, and no neutral stand-in would preserve the gate without
/// re-introducing the same write-hostility in the strict case. A deployment that
/// raises `duplicate_tolerance_write` is where the trade is visible.
///
/// # Scope
///
/// Meaningful only for a single-row read of stored content. It is not a general
/// "is this error ignorable" test, and applying it to an attribute read or to
/// any path where `Forbidden` means authorization would swallow an access-control
/// failure. Both call sites in this workspace are content reads.
#[must_use]
pub fn is_unreadable_content(error: &AmError) -> bool {
    matches!(
        error.code,
        ErrorCode::UnreadableContent | ErrorCode::Forbidden | ErrorCode::ErasedSubject
    )
}

#[cfg(test)]
#[path = "content_read_errors_tests.rs"]
mod content_read_errors_tests;

#[cfg(test)]
#[path = "content_read_errors_damage_tests.rs"]
mod content_read_errors_damage_tests;
