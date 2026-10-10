// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the sync model helpers: `DeadLetterStatus` column
//! encoding and parsing.
//!
//! ## Owns
//! - Unit coverage for `DeadLetterStatus` roundtrips
//!
//! ## Does not own
//! - Persistence behavior (`sqlite_sync` / `postgres_sync` tests)
//!
//! ## Boundary
//!
//! Tests here break when the sync status encoding changes; they never change it.
//!
//! Keep this module focused on sync model tests.

use crate::sync_store::DeadLetterStatus;

#[test]
fn dead_letter_status_roundtrip_and_unknown() {
    assert_eq!(DeadLetterStatus::Open.as_str(), "open");
    assert_eq!(DeadLetterStatus::AckedSkip.as_str(), "acked_skip");
    assert_eq!(
        DeadLetterStatus::parse("open"),
        Some(DeadLetterStatus::Open)
    );
    assert_eq!(
        DeadLetterStatus::parse("acked_skip"),
        Some(DeadLetterStatus::AckedSkip)
    );
    assert_eq!(DeadLetterStatus::parse("other"), None);
    assert_eq!(DeadLetterStatus::parse(""), None);
    assert_eq!(DeadLetterStatus::parse("ackeD_skip"), None);
}
