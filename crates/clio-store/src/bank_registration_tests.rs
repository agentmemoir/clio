// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that trigger coverage matches the legacy bank backfill.
//!
//! ## Owns
//! - The trigger-table/backfill-list contract
//!
//! ## Does not own
//! - Backend trigger behavior, covered by store and MCP tests
//!
//! ## Boundary
//!
//! Tests here break when a bank-scoped table is missing from either list.

use super::BANK_SCOPED_TABLES;
use crate::migrate::SQL_CORE;

#[test]
fn every_bank_trigger_table_is_in_the_legacy_backfill() {
    for table in BANK_SCOPED_TABLES {
        assert!(
            SQL_CORE.contains(&format!("SELECT bank_id FROM {table}")),
            "legacy bank backfill omits {table}"
        );
    }
}
