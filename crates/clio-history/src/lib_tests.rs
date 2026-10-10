// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the crate-level helpers and the
//! no-op telemetry sink.
//!
//! ## Owns
//! - `require_nonempty` / `require_bank_and_timestamp` behavior
//! - `NoopHistorySink` acceptance
//!
//! ## Does not own
//! - Any store-backed history behavior via the tools
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::{HistorySink, NoopHistorySink, require_bank_and_timestamp, require_nonempty};
use clio_types::{Actor, Bank, ErrorCode, OpContext};

#[test]
fn noop_sink_accepts_events() {
    let sink = NoopHistorySink;
    HistorySink::emit(
        &sink,
        crate::HistoryEvent {
            operation: "task_upsert".into(),
            outcome: "stored".into(),
            bank: "b".into(),
            actor: "a".into(),
            timestamp: "t".into(),
            task_id: "t".into(),
            record_id: "r".into(),
            record_count: 1,
            admission_score: None,
            rejection_reason: None,
        },
    );
}

#[test]
fn require_helpers_reject_empty_inputs() {
    assert_eq!(
        require_nonempty("", "x").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    let ctx = OpContext::new(Bank::new("bank"), Actor::Agent);
    assert_eq!(require_bank_and_timestamp(&ctx, "t").unwrap(), "bank");
    assert_eq!(
        require_bank_and_timestamp(&ctx, "").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    let empty = OpContext::new(Bank::new(""), Actor::Agent);
    assert_eq!(
        require_bank_and_timestamp(&empty, "t").unwrap_err().code,
        ErrorCode::MissingBank
    );
}
