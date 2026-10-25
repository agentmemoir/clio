// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one thing: which of the store's tables hold
//! derived, rebuildable rows rather than user data. It is the single place the
//! classification lives, so a caller that needs to know whether a table's rows
//! can be rebuilt asks here rather than keeping a list of its own.
//!
//! ## Owns
//! - `TableDataClass`, the two classes a table's rows fall in
//! - `DERIVED_TABLES`, the derived tables, each with why it is derived
//! - `table_data_class`, the one lookup, including what an unknown table is
//!
//! ## Does not own
//! - The schema itself, whose comments state the same thing per table (see
//!   `sql/001_core.sql`)
//! - What any caller does with the class (see the restore's post-load count
//!   comparison, which reports a difference rather than failing on one)
//!
//! ## Boundary
//!
//! Changes to which tables are derived should normally require changes here,
//! and nowhere else. Changes to how a caller treats a derived table should not.
//!
//! Keep this module focused on the classification.
//
// A table this module does not list is user data. That default is deliberate:
// an unknown table is the case where a difference must stay a hard failure, so
// nothing is derived by omission.

/// Which of the two kinds of row a table holds, from the store's own schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableDataClass {
    /// Rows a writer put there and nothing can rebuild: the store's content,
    /// its journals, and its registries. A restore that loads a different
    /// number of them has lost or invented data.
    UserData,
    /// Rows the store derives from user data by work it can repeat: a work
    /// queue or a rebuildable index. Wiping and rebuilding them loses nothing
    /// an operator wrote, so a count difference is not the same class of
    /// event as a user-data difference.
    Derived,
}

/// The derived application tables, with why each one is derived.
///
/// One entry today. Each is a table whose rows are a work queue the store
/// drains and refills from the tables beside it, never a record of what an
/// operator wrote.
const DERIVED_TABLES: &[(&str, &str)] = &[(
    "index_pending",
    "the durable outbox of leaves awaiting dense or lexical indexing: one row per \
     queued (bank, item), drained off the write path by the index worker and \
     rebuildable from items",
)];

/// The class of one table's rows, by table name.
///
/// A table this build does not list is `UserData`, including a table added by a
/// newer schema than this build knows: the classification can only vouch for
/// tables it names, so everything else keeps the stricter treatment.
#[must_use]
pub fn table_data_class(table: &str) -> TableDataClass {
    if DERIVED_TABLES.iter().any(|(name, _)| *name == table) {
        TableDataClass::Derived
    } else {
        TableDataClass::UserData
    }
}

#[cfg(test)]
#[path = "derived_tables_tests.rs"]
mod tests;
