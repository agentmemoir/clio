// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the index report shapes and the
//! row-loss classification they count.
//!
//! ## Owns
//! - `classify_row_loss` mapping and the report counter defaults
//!
//! ## Does not own
//! - Sweep or worker behavior that fills the counters (see `rebuild_tests`/`worker_tests`)
//! - Backend coverage SQL (`clio_store::IndexStore`)
//!
//! ## Boundary
//!
//! Changes to the report shapes or the row-loss mapping should normally update
//! tests here.
//! Changes to sweep or worker policy should not require changes here.
//!
//! Keep this module focused on the report value shapes.

use crate::report::{ProcessReport, RebuildReport, RowLoss, classify_row_loss};
use clio_types::{AmError, ErrorCode};

/// Damage stays apart from expected erasure, and a backend fault is neither.
#[test]
fn row_loss_separates_damage_from_erasure() {
    for code in [ErrorCode::UnreadableContent, ErrorCode::Forbidden] {
        assert_eq!(
            classify_row_loss(&AmError::new(code, "content decryption failed")),
            Some(RowLoss::Unreadable),
            "{code:?} is row damage"
        );
    }
    assert_eq!(
        classify_row_loss(&AmError::new(
            ErrorCode::ErasedSubject,
            "DEK for subject has been destroyed"
        )),
        Some(RowLoss::Erased),
        "a crypto-shredded subject is an expected loss"
    );
    assert_eq!(
        classify_row_loss(&AmError::new(ErrorCode::Internal, "driver is down")),
        None,
        "a backend fault must stay fatal"
    );
}

/// Both reports start with zero row-loss counters.
#[test]
fn report_counters_default_to_zero() {
    let process = ProcessReport::default();
    assert_eq!((process.unreadable, process.erased), (0, 0));
    let rebuild = RebuildReport::default();
    assert_eq!((rebuild.unreadable, rebuild.erased), (0, 0));
}
