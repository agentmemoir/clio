// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one thing: the Postgres restore's post-load
//! count comparison. It decides whether the observation the store half made
//! inside the restore transaction is a clean restore, a user-data difference,
//! a derived-table difference, or a partial restore, and it words the two
//! difference reports.
//!
//! ## Owns
//! - `result_for`, the comparison's outcome
//! - `count_mismatch_message` and `derived_difference_message`, the two reports
//! - `difference_message`, the shared per-table wording
//!
//! ## Does not own
//! - Which table holds derived rows (see `clio_store`'s derived-table
//!   classification, the one place that list lives)
//! - Which exit code a result maps to (see `backup_restore_exit`)
//! - The measurement itself (see `clio_store`'s one-transaction restore)
//!
//! ## Boundary
//!
//! Changes to how a count difference is judged or worded should normally
//! require changes here. Changes to the restore's sequence should not.
//!
//! Keep this module focused on the comparison.

use crate::backup_restore_flow::{
    RESULT_COUNT_MISMATCH, RESULT_DERIVED_DIFFERENT, RESULT_PARTIAL, RESULT_RESTORED,
};
use crate::count_noun::count_noun;
use clio_backup::dump_meta::DumpMeta;
use clio_store::TableDataClass;

/// The result one placement produced.
///
/// A user-data count difference is the failed check it has always been, and a
/// key store that was not placed is a partial restore. A difference confined to
/// a derived table is neither: every user-data row arrived, so it gets its own
/// result and its own exit code rather than a failed step.
pub(crate) fn result_for(
    mismatch: Option<&str>,
    derived: Option<&str>,
    key_failure: Option<&str>,
) -> &'static str {
    if mismatch.is_some() {
        RESULT_COUNT_MISMATCH
    } else if key_failure.is_some() {
        RESULT_PARTIAL
    } else if derived.is_some() {
        RESULT_DERIVED_DIFFERENT
    } else {
        RESULT_RESTORED
    }
}

/// The user-data tables whose observed counts differ from the archive's, when
/// any do. The message names only tables and counts, never row content.
pub(crate) fn count_mismatch_message(
    meta: &DumpMeta,
    observed: &[(String, u64)],
) -> Option<String> {
    difference_message(
        meta,
        observed,
        TableDataClass::UserData,
        "the restored table counts do not match the archive's:",
    )
}

/// The derived tables whose observed counts differ, when any do.
///
/// This is not the count mismatch: every user-data row loaded, and the store
/// rebuilds these rows from it, so the text says both things and names what
/// differs, so an operator can tell work a rebuild will do from a restore that
/// lost data.
pub(crate) fn derived_difference_message(
    meta: &DumpMeta,
    observed: &[(String, u64)],
) -> Option<String> {
    difference_message(
        meta,
        observed,
        TableDataClass::Derived,
        "every user-data row was loaded; a table the store rebuilds differs:",
    )
}

/// One class's differing tables, joined behind the sentence that introduces
/// them, or `None` when every table of that class matches the archive.
fn difference_message(
    meta: &DumpMeta,
    observed: &[(String, u64)],
    class: TableDataClass,
    lead: &str,
) -> Option<String> {
    let differing: Vec<String> = meta
        .tables
        .iter()
        .filter(|table| clio_store::table_data_class(&table.table) == class)
        .filter_map(|table| {
            let seen = observed
                .iter()
                .find(|(name, _)| name == &table.table)
                .map(|(_, count)| *count);
            match seen {
                Some(count) if count == table.rows => None,
                Some(count) => Some(format!(
                    "table {:?}: archive {}, restored {count}",
                    table.table,
                    count_noun(table.rows, "row", "rows")
                )),
                None => Some(format!(
                    "table {:?}: archive {}, restored: not counted",
                    table.table,
                    count_noun(table.rows, "row", "rows")
                )),
            }
        })
        .collect();
    if differing.is_empty() {
        return None;
    }
    Some(format!("{lead} {}", differing.join("; ")))
}

/// The comparison's wording and outcome, over hand-built facts.
#[cfg(test)]
#[path = "backup_restore_counts_tests.rs"]
mod tests;
