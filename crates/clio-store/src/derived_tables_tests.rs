// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the derived-table classification's own
//! coverage: that the one derived table is derived, that every user-data table
//! the store writes is not, and that a table the classification does not name
//! falls on the user-data side.
//!
//! ## Owns
//! - The classification's positive and negative cases, and its default
//!
//! ## Does not own
//! - What a caller does with the class (see the restore's count comparison)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every test here is pure.
//!
//! Keep this module focused on the classification.

use super::{DERIVED_TABLES, TableDataClass, table_data_class};

#[test]
fn the_work_queue_is_the_derived_table() {
    assert_eq!(table_data_class("index_pending"), TableDataClass::Derived);
    let named: Vec<&str> = DERIVED_TABLES.iter().map(|(name, _)| *name).collect();
    assert_eq!(named, vec!["index_pending"]);
}

/// Every entry states why it is derived, so the list cannot grow by name alone.
#[test]
fn every_derived_table_says_why_it_is_derived() {
    for (name, reason) in DERIVED_TABLES {
        assert_ne!(*name, "", "a derived table needs a name to classify");
        assert!(
            reason.len() > 20,
            "{name} needs a real reason, not a placeholder"
        );
    }
}

/// The user-data tables the store's own schema defines: a derived class here
/// would let a restore report a lost row as a rebuildable difference.
#[test]
fn the_stores_user_data_tables_are_not_derived() {
    for table in [
        "items",
        "triples",
        "banks",
        "beliefs",
        "audit_trail",
        "attribute_schemas",
        "continuous_state",
        "local_events",
        "peer_actions",
    ] {
        assert_eq!(
            table_data_class(table),
            TableDataClass::UserData,
            "{table} holds user data"
        );
    }
}

/// A table the classification does not name is user data, including one a
/// newer schema added: the default is the strict side, so an unknown table
/// keeps a difference a hard failure.
#[test]
fn a_table_the_code_does_not_classify_is_user_data() {
    assert_eq!(
        table_data_class("a_table_from_a_newer_schema"),
        TableDataClass::UserData
    );
    assert_eq!(table_data_class(""), TableDataClass::UserData);
}
