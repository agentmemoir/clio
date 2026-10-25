// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the derived-table classification's own
//! coverage: that the classification names exactly the tables the schema marks
//! derived, that every user-data table the store writes is not derived, and that
//! a table the classification does not name falls on the user-data side.
//!
//! ## Owns
//! - The classification's positive and negative cases, against the schema's own
//!   markers rather than a list copied into this file
//! - The default for a table the classification does not name
//!
//! ## Does not own
//! - What a caller does with the class (see the restore's count comparison)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every test here is pure:
//! the schema text is embedded at compile time, so nothing is read from the host.
//!
//! Keep this module focused on the classification.

use super::{DERIVED_TABLES, TableDataClass, table_data_class};
use crate::migrate::SQL_CORE;

/// The tables the schema itself marks derived, sorted.
///
/// `sql/001_core.sql` states each table's class in the comment above its
/// `CREATE`: the work queue carries `Derived state`, and nothing else does. That
/// comment is the schema's own answer, so the classification is checked against
/// it and not against a second copy of the list written here, which would let a
/// table the schema marks derived stay silent when the list missed it.
fn schema_derived_tables() -> Vec<String> {
    let mut named: Vec<String> = SQL_CORE
        .split("Derived state")
        .skip(1)
        .filter_map(|after_marker| {
            // The marker sits in the comment block directly above its table.
            let after_create = after_marker.split("CREATE TABLE IF NOT EXISTS ").nth(1)?;
            let name: String = after_create
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            assert!(
                !name.is_empty(),
                "the schema marks a derived table but no CREATE follows it"
            );
            Some(name)
        })
        .collect();
    named.sort_unstable();
    named
}

/// The classification and the schema agree on which tables are derived, in both
/// directions: a derived table the schema added and the list missed fails here,
/// and so does a list entry the schema does not mark.
#[test]
fn the_classification_matches_the_tables_the_schema_marks_derived() {
    let mut named: Vec<String> = DERIVED_TABLES
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect();
    named.sort_unstable();

    assert_eq!(
        named,
        schema_derived_tables(),
        "the derived list and the schema's own markers must agree"
    );
    for name in &named {
        assert_eq!(
            table_data_class(name),
            TableDataClass::Derived,
            "{name} is marked derived in the schema, so the lookup must classify it so"
        );
    }
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
