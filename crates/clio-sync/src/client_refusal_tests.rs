// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the operator-facing refusal reasons the sync receive path
//! records: each one names its real cause and the deliberate recovery, and the
//! sealed-mode refusal says what the node accepts instead.
//!
//! ## Owns
//! - The wording contract of the three receive-path refusals
//!
//! ## Does not own
//! - How a refusal quarantines a row or freezes the cursor (`client_*_tests`)
//!
//! ## Boundary
//!
//! These tests break when a refusal reason is reworded; they never reword one.
//!
//! Keep this module focused on the refusal wording.

use crate::client_refusal::{PLAINTEXT_REFUSED_REASON, missing_key_reason, open_failed_reason};

#[test]
fn the_sealed_mode_refusal_names_what_the_node_accepts() {
    assert!(
        PLAINTEXT_REFUSED_REASON.starts_with("sealed mode refused plaintext content"),
        "the reason leads with the cause: {PLAINTEXT_REFUSED_REASON}"
    );
    assert!(
        PLAINTEXT_REFUSED_REASON.contains("accepts only sealed content"),
        "it says what the node accepts: {PLAINTEXT_REFUSED_REASON}"
    );
    assert!(
        PLAINTEXT_REFUSED_REASON.contains("clio sync ack-skip"),
        "it names the recovery: {PLAINTEXT_REFUSED_REASON}"
    );
}

#[test]
fn the_missing_key_refusal_names_the_stamped_key_and_the_held_ones() {
    let held = vec!["key-one".to_owned(), "key-two".to_owned()];
    let reason = missing_key_reason("stamped-key", &held);
    assert!(
        reason.contains("sealed under key stamped-key"),
        "it names the key that sealed the row: {reason}"
    );
    assert!(
        reason.contains("held keys: key-one, key-two"),
        "it names every key this node holds: {reason}"
    );
    assert!(
        reason.contains("clio sync ack-skip"),
        "it names the recovery: {reason}"
    );
}

#[test]
fn the_failed_open_refusal_names_the_opening_key_and_the_damaged_bytes() {
    let reason = open_failed_reason("stamped-key");
    assert!(
        reason.contains(
            "stamped with key stamped-key could not be opened with the matching held key"
        ),
        "it names the key that failed to open the row: {reason}"
    );
    assert!(
        reason.contains("may be corrupted or tampered with"),
        "it says the bytes are untrustworthy: {reason}"
    );
}
