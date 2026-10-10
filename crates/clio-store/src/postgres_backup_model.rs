// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the pure pieces the Postgres backup halves
//! share: the table and column shapes discovery produces, SQL identifier
//! quoting, the dump member naming, and the table-set comparison a restore's
//! pre-flight runs.
//!
//! ## Owns
//! - `PgTable` / `PgColumn`, the shapes every backup half passes around
//! - `quote_ident`, one quoted SQL identifier from one name
//! - `member_name`, the `dump/tables/NNN_<table>.txt` member numbering
//! - `compare_table_sets`, the pure pre-flight refusal for a mismatched target
//!
//! ## Does not own
//! - Catalog queries or transactions (see `postgres_discovery`, `postgres_dump`,
//!   and `postgres_restore`)
//! - The archive's `dump/meta.json` type (see `clio-backup`)
//! - The SQLite snapshot halves (see `sqlite_snapshot`, `restore_state`)
//!
//! ## Boundary
//!
//! Changes to what discovery reports about a table should normally require
//! changes here. Changes to how the catalog is queried or how a member is
//! staged should not require changes here.
//!
//! Keep this module focused on the pure backup-half contract.

use clio_types::{AmError, ErrorCode};

/// One column of one application table, in catalog order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PgColumn {
    /// The column name, unquoted.
    pub name: String,
    /// The column's ordinal position, one-based.
    pub ordinal: i32,
    /// The column's type, as Postgres would print it (`text`, `vector(3)`).
    pub type_name: String,
}

/// One application table: its name and its ordered columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PgTable {
    /// The table name, unquoted.
    pub name: String,
    /// The table's columns, ordered by ordinal position.
    pub columns: Vec<PgColumn>,
}

impl PgTable {
    /// The table's column names, in ordinal order. This is the list a
    /// `COPY ... (cols)` statement names and the list the table-set
    /// comparison checks.
    #[must_use]
    pub fn column_names(&self) -> Vec<String> {
        self.columns.iter().map(|c| c.name.clone()).collect()
    }
}

/// One quoted SQL identifier, safe for a table or column the catalog or the
/// archive named.
///
/// The name is never taken from operator input: discovery reads it from the
/// catalog and the restore reads it from the archive's own dump metadata.
/// Doubling embedded quotes is the whole job; nothing else can escape the
/// quoted form.
#[must_use]
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The dump member name for the table at one-based `ordinal`.
///
/// Members are numbered in the discovered order, which is sorted by table
/// name, so two snapshots of an unchanged store produce the same member names.
///
/// # Errors
/// The ordinal is past the three-digit numbering, the table name is empty, or
/// the name the table would produce cannot be an archive member (a separator,
/// a non-printable byte, or a name over the container's limit).
pub fn member_name(ordinal: usize, table: &str) -> Result<String, AmError> {
    if table.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "a dump table member cannot be named for an empty table name".to_owned(),
        ));
    }
    if ordinal == 0 || ordinal > 999 {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("dump table member numbering runs 001..=999, got {ordinal}"),
        ));
    }
    if table.contains('/') || table.contains('\\') {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!(
                "a dump table member cannot be named for the table {table:?}: a table name \
                     with a path separator would not be the flat member the numbering promises"
            ),
        ));
    }
    // The same rules the archive container's member-name gate enforces, applied
    // here so a table name that could not ride in an archive is refused before
    // any bytes are staged rather than mid-dump. The container gate runs again
    // at write time, so the two can never disagree about an archive that was
    // actually written.
    let name = format!("dump/tables/{ordinal:03}_{table}.txt");
    validate_member_name(&name)?;
    Ok(name)
}

/// Compare the archive's table set with the target's discovered set.
///
/// Both sides are `(table name, ordered column names)` pairs. Three rules:
/// a table the archive carries and the target does not refuses, a table the
/// target has and the archive does not refuses, and a target table that is
/// missing a column the archive records refuses. A target column the archive
/// does not record is fine: the load names its columns explicitly, so an
/// added, defaulted target column cannot shift a positional load.
///
/// Both sets are named in the refusal, so an operator can see the drift
/// without opening anything.
///
/// # Errors
/// Either set disagrees with the other, per the rules above.
pub fn compare_table_sets(
    target: &[(String, Vec<String>)],
    archive: &[(String, Vec<String>)],
) -> Result<(), AmError> {
    let refused = |message: String| Err(AmError::new(ErrorCode::InvalidArgument, message));
    let target_names: Vec<&str> = target.iter().map(|(name, _)| name.as_str()).collect();
    let archive_names: Vec<&str> = archive.iter().map(|(name, _)| name.as_str()).collect();
    for (name, columns) in archive {
        if !target_names.contains(&name.as_str()) {
            return refused(format!(
                "the archive's table set does not match the target: the archive carries \
                 {name:?}, the target does not (archive: {archive_names:?}; target: \
                 {target_names:?})"
            ));
        }
        let Some(target_columns) = target.iter().find(|(t, _)| t == name).map(|(_, c)| c) else {
            continue;
        };
        for column in columns {
            if !target_columns.contains(column) {
                return refused(format!(
                    "the archive's table set does not match the target: the target's table \
                     {name:?} has no column {column:?} (archive: {archive_names:?}; target: \
                     {target_names:?})"
                ));
            }
        }
    }
    for name in &target_names {
        if !archive_names.contains(name) {
            return refused(format!(
                "the archive's table set does not match the target: the target has {name:?}, \
                 the archive does not (archive: {archive_names:?}; target: {target_names:?})"
            ));
        }
    }
    Ok(())
}

/// The longest member name the archive container accepts.
const MAX_MEMBER_NAME_BYTES: usize = 100;

/// The same member-name rules the archive container enforces, so a table name
/// that could not be an archive member is refused here, before bytes are
/// staged. Non-empty, no leading or embedded separators that escape the
/// member, printable ASCII only, and within the container's name limit.
fn validate_member_name(name: &str) -> Result<(), AmError> {
    let reject = |reason: String| {
        Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("invalid dump member name {name:?}: {reason}"),
        ))
    };
    if name.len() > MAX_MEMBER_NAME_BYTES {
        return reject(format!(
            "the name is {} bytes, over the {MAX_MEMBER_NAME_BYTES}-byte limit",
            name.len()
        ));
    }
    if let Some(&byte) = name
        .as_bytes()
        .iter()
        .find(|&&b| !(0x20..=0x7E).contains(&b))
    {
        return reject(format!(
            "byte 0x{byte:02x} is outside printable ASCII (0x20..=0x7E)"
        ));
    }
    for segment in name.split('/') {
        match segment {
            "" | "." | ".." => {
                return reject(format!("the name has a {segment:?} path segment"));
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "postgres_backup_model_tests.rs"]
mod tests;
