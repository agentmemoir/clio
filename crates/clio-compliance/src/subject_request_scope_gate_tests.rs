// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the request scope gate: a record's
//! own bank passes, and a caller from another bank is refused with the shared
//! refusal while the refused attempt reaches the audit sink.
//!
//! ## Owns
//! - The own-bank pass and the foreign-bank refusal of `require_scope_bank`
//! - The proof that a refusal reaches the audit sink instead of passing silently
//!
//! ## Does not own
//! - The verbs that call the gate (`subject_request_service`, tested by its own
//!   suites)
//! - The record and context fixtures (`subject_request_service_fixtures`)
//!
//! ## Boundary
//!
//! Changes to what the gate allows, refuses, or records should normally require
//! changes here. Changes to a request verb should not.
//!
//! Test-only. Never compiled into production builds.

use crate::subject_request::RequestState;
use crate::subject_request_scope_gate::require_scope_bank;
use crate::subject_request_service_fixtures::{ctx, record, store};
use clio_types::ErrorCode;

/// A caller inside the record's own bank passes the gate.
#[test]
fn the_records_own_bank_passes() {
    let record = record("req-gate-own-bank", RequestState::InProgress);
    require_scope_bank(&store(), &record, &ctx("bank-a"), "subject_request_close")
        .expect("the owning bank passes");
}

/// A caller outside the record's bank is refused with the refusal every scoped
/// verb shares.
#[test]
fn a_foreign_bank_is_refused() {
    let record = record("req-gate-foreign-bank", RequestState::InProgress);
    let err = require_scope_bank(
        &store(),
        &record,
        &ctx("bank-b"),
        "subject_request_apply_command",
    )
    .expect_err("another bank is refused");
    assert_eq!(err.code, ErrorCode::Forbidden, "{err:?}");
    assert_eq!(err.message, "request belongs to another bank");
}

/// A refusal reaches the audit sink: when the sink cannot be appended to, the
/// gate reports the sink's own error rather than a silent refusal, so the audit
/// write is on the refusal path and not decoration.
#[test]
fn a_refusal_appends_to_the_audit_sink() {
    let store = store();
    let record = record("req-gate-audit", RequestState::InProgress);
    // The empty operation is the one input the audit sink refuses, so a gate
    // that skipped the write would return `Forbidden` here instead.
    let err = require_scope_bank(&store, &record, &ctx("bank-b"), "")
        .expect_err("a refusal with an unusable audit label reports the sink error");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err:?}");
}
