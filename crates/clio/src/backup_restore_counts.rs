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
//! - `comparison`, the comparison's outcome
//! - `classify`, the one statement of what a difference in either direction
//!   means for either class of table
//! - `count_mismatch_message` and `derived_difference_message`, the two reports
//! - `differing_tables`, the shared per-table wording
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

/// What one table's observation means for the restore.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The restored count equals the archive's.
    Clean,
    /// Rows the archive holds and the restore did not load, or rows the load
    /// invented: the failed count check.
    HardMismatch,
    /// A table the store rebuilds that came back with more rows than the
    /// archive recorded: work a rebuild absorbs, so not a failure.
    DerivedSurplus,
}

/// The rule, in one place, for every table of either class.
///
/// The measurement is taken inside the restore's own transaction, so nothing
/// else can land a row between the load and the count. What is left is the load
/// itself: a restored count below the archive's is a row the archive held and
/// the restore did not load, and above the archive's it is a row the load
/// invented. Both are the hard count mismatch, for a table the store rebuilds
/// as much as for a user-data table. The single exception is a surplus in a
/// derived table: those rows are derived from the user data the restore placed,
/// so a rebuild replaces them and the difference is not a failure. A table the
/// target never counted proves nothing either way, so it is a hard mismatch.
fn classify(class: TableDataClass, seen: Option<u64>, archived: u64) -> Outcome {
    match seen {
        Some(restored) if restored == archived => Outcome::Clean,
        Some(restored) if restored > archived && class == TableDataClass::Derived => {
            Outcome::DerivedSurplus
        }
        None | Some(_) => Outcome::HardMismatch,
    }
}

/// The comparison's outcome: the result, the hard count mismatch behind it
/// when there is one, and the benign derived difference beside it when there is
/// one. This is the one place an observation becomes an answer, so the messages
/// the report carries and the result the exit code reads cannot disagree.
pub(crate) fn comparison(
    meta: &DumpMeta,
    observed: &[(String, u64)],
    key_failure: Option<&str>,
) -> (&'static str, Option<String>, Option<String>) {
    let mismatch = count_mismatch_message(meta, observed);
    let derived = derived_difference_message(meta, observed);
    let result = result_for(mismatch.as_deref(), derived.as_deref(), key_failure);
    (result, mismatch, derived)
}

/// The result one placement produced.
///
/// A user-data count difference is the failed check it has always been, and a
/// key store that was not placed is a partial restore. A difference confined to
/// a derived table is neither: every user-data row arrived, so it gets its own
/// result and its own exit code rather than a failed step.
fn result_for(
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

/// The hard count mismatches, when any table came back with a count the archive
/// does not support. The message names only tables and counts, never row
/// content.
pub(crate) fn count_mismatch_message(
    meta: &DumpMeta,
    observed: &[(String, u64)],
) -> Option<String> {
    differing_tables(
        meta,
        observed,
        Outcome::HardMismatch,
        "the restored table counts do not match the archive's:",
    )
}

/// The derived tables the restore over-filled, when any. This is not the count
/// mismatch: every user-data row loaded, and the store rebuilds these rows from
/// it, so the text says both things and names what differs, so an operator can
/// tell work the index pipeline will do from a restore that lost data. It also
/// says what does not clear the surplus: the reindex a restore prints writes the
/// vector and lexical index and never the work queue, and a retry of the restore
/// reloads the same archive and reproduces the same count.
pub(crate) fn derived_difference_message(
    meta: &DumpMeta,
    observed: &[(String, u64)],
) -> Option<String> {
    differing_tables(
        meta,
        observed,
        Outcome::DerivedSurplus,
        "every user-data row was loaded; a table the store rebuilds holds a surplus that neither \
         the reindex a restore prints nor a retry of the restore clears, and the index pipeline \
         reconciles it on its next pass:",
    )
}

/// The tables the rule put in `want`, joined behind the sentence that
/// introduces them, or `None` when no table landed there.
fn differing_tables(
    meta: &DumpMeta,
    observed: &[(String, u64)],
    want: Outcome,
    lead: &str,
) -> Option<String> {
    let differing: Vec<String> = meta
        .tables
        .iter()
        .filter_map(|table| {
            let seen = observed
                .iter()
                .find(|(name, _)| name == &table.table)
                .map(|(_, count)| *count);
            let class = clio_store::table_data_class(&table.table);
            if classify(class, seen, table.rows) != want {
                return None;
            }
            let rows = count_noun(table.rows, "row", "rows");
            Some(match seen {
                Some(count) => format!("table {:?}: archive {rows}, restored {count}", table.table),
                None => format!(
                    "table {:?}: archive {rows}, restored: not counted",
                    table.table
                ),
            })
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
