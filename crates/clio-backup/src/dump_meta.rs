// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `dump/meta.json` member of a Postgres
//! archive: the dump's own description of its schema, table set, and per-table
//! members, with the parsing and validation every reader of the member runs.
//!
//! ## Owns
//! - `DumpMeta` / `DumpTable` / `DumpColumn` and their JSON (de)serialization
//! - The member's validation rules: kind and format version, the table set's
//!   deterministic order, member-name, digest, and declared-size checks, and
//!   column lists
//! - `DumpMeta::read_from_archive`: reading the member out of one archive
//!   without inflating the table members
//! - `validate_dump_meta_file`: the integrity check a verifier runs over the
//!   member once it has been staged
//!
//! ## Does not own
//! - Producing the dump (see `clio-store`'s postgres dump half)
//! - The manifest type (see `manifest`)
//! - Container bounds and encoding (see `container`)
//!
//! ## Boundary
//!
//! Changes to what a dump records about itself should normally require changes
//! here. Changes to the container or the manifest should not require changes
//! here.
//!
//! Keep this module focused on the dump meta member.

use crate::container::{ArchiveReader, ReaderBounds};
use crate::name::validate_member_name;
use clio_types::{AmError, ErrorCode};
use std::io::Read;
use std::path::Path;

/// The member name the dump's own description rides in; the manifest's payload
/// member for a Postgres archive, written right after the manifest.
pub const DUMP_META_MEMBER: &str = "dump/meta.json";

/// The member-name prefix every dumped table's member carries.
pub const DUMP_TABLES_PREFIX: &str = "dump/tables/";

/// The dump meta `kind` value this build reads and writes.
pub const DUMP_KIND: &str = "clio-store-dump";

/// The dump meta format version this build supports.
pub const DUMP_FORMAT_VERSION: u32 = 1;

/// The largest `dump/meta.json` this build reads. The member is metadata: its
/// size grows with the table and column counts, not with the rows, so one
/// mebibyte bounds it generously.
pub const MAX_DUMP_META_BYTES: u64 = 1024 * 1024;

/// One column of one dumped table, in dump order.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DumpColumn {
    /// The column name, unquoted.
    pub name: String,
    /// The column's ordinal position, one-based, strictly increasing.
    pub ordinal: i32,
    /// The column's type, as Postgres would print it.
    pub type_name: String,
}

/// One dumped table and the member its rows ride in.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DumpTable {
    /// The table name, unquoted.
    pub table: String,
    /// The member the rows were streamed into.
    pub member: String,
    /// Bytes of the member.
    pub bytes: u64,
    /// Lowercase-hex SHA-256 of the member.
    pub sha256: String,
    /// Rows copied, counted while streaming.
    pub rows: u64,
    /// The table's columns, in dump order.
    pub columns: Vec<DumpColumn>,
}

/// The `dump/meta.json` member: what the dump read, and where each table's
/// rows are.
///
/// Field order is the serialization order, so identical dumps serialize to
/// identical bytes. Unknown fields are ignored on read, so the field set can
/// grow without a version bump.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DumpMeta {
    pub kind: String,
    pub format_version: u32,
    /// The backend the dump came from (`postgres`).
    pub backend: String,
    /// The schema the dump read.
    pub schema: String,
    /// The database name, credential-free.
    pub database: String,
    /// The installed extensions, sorted by name.
    pub extensions: Vec<String>,
    /// The distinct bank ids of the `items` table, sorted.
    pub banks: Vec<String>,
    /// How the dump was taken.
    pub method: String,
    /// The consistency the mechanism gives.
    pub consistency: String,
    /// Every dumped table, in member order (name order).
    pub tables: Vec<DumpTable>,
}

impl DumpMeta {
    /// Serialize to deterministic JSON bytes.
    pub fn to_json_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("dump meta serialization cannot fail")
    }

    /// Parse and validate dump meta JSON bytes.
    ///
    /// # Errors
    /// The bytes are not JSON of this kind and version, or the member fails
    /// its own validation.
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self, AmError> {
        let meta: DumpMeta = serde_json::from_slice(bytes).map_err(|error| {
            AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!("invalid {DUMP_META_MEMBER} JSON: {error}"),
            )
        })?;
        meta.validate()?;
        Ok(meta)
    }

    /// Read `dump/meta.json` out of one archive, without inflating any table
    /// member.
    ///
    /// # Errors
    /// The archive is missing, unreadable, carries no such member, or carries
    /// a member this build refuses.
    pub fn read_from_archive(path: &Path) -> Result<Self, AmError> {
        let file = std::fs::File::open(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                AmError::new(
                    ErrorCode::NotFound,
                    format!("no snapshot archive at {}", path.display()),
                )
            } else {
                AmError::new(
                    ErrorCode::Internal,
                    format!("opening {}: {error}", path.display()),
                )
            }
        })?;
        let bounds = ReaderBounds {
            // Manifest, payload, and key member: the three members any legal
            // archive carries before its table members. A larger cap cannot be
            // tripped by an archive this build produced, and a smaller one would
            // answer a member-count refusal for an archive that simply carries
            // no dump meta, which is the answer the operator needs.
            max_members: 3,
            max_member_bytes: MAX_DUMP_META_BYTES,
            max_total_bytes: MAX_DUMP_META_BYTES,
        };
        let mut reader = ArchiveReader::with_bounds(file, bounds)?;
        while let Some(header) = reader.next_member()? {
            if header.name == DUMP_META_MEMBER {
                let mut bytes = Vec::new();
                reader.read_current(&mut bytes)?;
                return DumpMeta::from_json_bytes(&bytes);
            }
            reader.skip_current()?;
        }
        Err(AmError::new(
            ErrorCode::ArchiveCorrupt,
            format!(
                "the archive at {} carries no {DUMP_META_MEMBER} member, so it is not a \
                 Postgres dump this build can read",
                path.display()
            ),
        ))
    }

    /// The member bytes in total, across every table.
    ///
    /// The counts are archive-supplied, so the total is accumulated with a
    /// saturating add: it can only reach the sum a caller needs as a size
    /// estimate, and a wrapped total would be a small number that reads as
    /// "this dump is tiny". `validate` also refuses a per-table count over the
    /// container's per-member cap, so a validated meta cannot get near the
    /// saturation point in the first place.
    #[must_use]
    pub fn table_member_bytes(&self) -> u64 {
        self.tables
            .iter()
            .fold(0u64, |total, table| total.saturating_add(table.bytes))
    }

    /// Refuse a dump meta this build cannot trust.
    ///
    /// # Errors
    /// Wrong kind, unsupported format version, wrong backend, an empty
    /// required field, a table set that is not sorted and unique, a member
    /// name the container would reject, a digest that is not 64 lowercase hex
    /// characters, a table byte count over the container's per-member cap, a
    /// column list that is empty or not ordinal-ordered and unique, or tables
    /// whose names repeat.
    pub fn validate(&self) -> Result<(), AmError> {
        if self.kind != DUMP_KIND {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!(
                    "unsupported dump meta kind {:?}; this build expects {DUMP_KIND:?}",
                    self.kind
                ),
            ));
        }
        if self.format_version != DUMP_FORMAT_VERSION {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!(
                    "unsupported dump meta format_version {}; this build supports \
                     {DUMP_FORMAT_VERSION}",
                    self.format_version
                ),
            ));
        }
        if self.backend != "postgres" {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!(
                    "unknown dump meta backend {:?}; this build supports \"postgres\"",
                    self.backend
                ),
            ));
        }
        for (value, field) in [
            (&self.schema, "schema"),
            (&self.database, "database"),
            (&self.method, "method"),
            (&self.consistency, "consistency"),
        ] {
            if value.is_empty() {
                return Err(AmError::new(
                    ErrorCode::ArchiveCorrupt,
                    format!("dump meta field {field} is empty"),
                ));
            }
        }
        if self.tables.is_empty() {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                "dump meta carries no tables; a store always has tables, so this is not a \
                 dump this build can restore",
            ));
        }
        let mut previous: Option<&str> = None;
        for table in &self.tables {
            Self::validate_table(table)?;
            if let Some(previous) = previous
                && table.table.as_str() <= previous
            {
                return Err(AmError::new(
                    ErrorCode::ArchiveCorrupt,
                    format!(
                        "dump meta tables are not in strictly ascending name order at \
                             {:?}; a deterministic dump is name-ordered",
                        table.table
                    ),
                ));
            }
            previous = Some(table.table.as_str());
        }
        Ok(())
    }

    /// The rules one dumped table must hold.
    fn validate_table(table: &DumpTable) -> Result<(), AmError> {
        if table.table.is_empty() {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                "dump meta has a table with an empty name",
            ));
        }
        validate_member_name(&table.member).map_err(|error| {
            AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!("dump meta table {:?}: {}", table.table, error.message),
            )
        })?;
        // The member suffix is compared by bytes on purpose: the numbering is
        // exact, and a `.TXT` member is not this build's member.
        if !table.member.starts_with("dump/tables/") || !table.member.as_bytes().ends_with(b".txt")
        {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!(
                    "dump table member {:?} is not a table member name this build numbers",
                    table.member
                ),
            ));
        }
        if table.sha256.len() != 64
            || table
                .sha256
                .bytes()
                .any(|b| !matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!(
                    "dump table member {:?} records a digest that is not 64 lowercase hex \
                     characters",
                    table.member
                ),
            ));
        }
        if table.columns.is_empty() {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!(
                    "dump table {:?} records no columns; the column list is what makes the \
                     load positional-safe",
                    table.table
                ),
            ));
        }
        // The declared size is bounded by the container's own per-member cap,
        // so a meta cannot claim a table member larger than any reader would
        // ever inflate. This is the same constant the reader enforces, read
        // from the default bounds rather than repeated here.
        let max_member_bytes = ReaderBounds::default().max_member_bytes;
        if table.bytes > max_member_bytes {
            return Err(AmError::new(
                ErrorCode::ArchiveCorrupt,
                format!(
                    "dump table member {:?} records {} bytes, over the per-member cap of {} \
                     bytes this build reads",
                    table.member, table.bytes, max_member_bytes
                ),
            ));
        }
        let mut last_ordinal: Option<i32> = None;
        for column in &table.columns {
            if column.name.is_empty() {
                return Err(AmError::new(
                    ErrorCode::ArchiveCorrupt,
                    format!(
                        "dump table {:?} has a column with an empty name",
                        table.table
                    ),
                ));
            }
            if let Some(last) = last_ordinal
                && column.ordinal <= last
            {
                return Err(AmError::new(
                    ErrorCode::ArchiveCorrupt,
                    format!(
                        "dump table {:?} columns are not in strictly ascending ordinal \
                             order at {:?}",
                        table.table, column.name
                    ),
                ));
            }
            last_ordinal = Some(column.ordinal);
        }
        Ok(())
    }
}

/// Read one file's bytes for a dump-meta parse, refusing an over-bound file
/// before any byte is read.
pub(crate) fn read_bounded_meta_bytes(path: &Path) -> Result<Vec<u8>, AmError> {
    let file = std::fs::File::open(path).map_err(|error| {
        AmError::new(
            ErrorCode::Internal,
            format!("reading the staged {DUMP_META_MEMBER}: {error}"),
        )
    })?;
    // The read is bounded by `take`, not by a pre-read size check: a file that
    // cannot report its size still reads, and a file that grows between the
    // check and the read is cut off at the bound either way.
    let mut bytes = Vec::new();
    let mut file = file;
    (&mut file)
        .take(MAX_DUMP_META_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            AmError::new(
                ErrorCode::Internal,
                format!("reading the staged {DUMP_META_MEMBER}: {error}"),
            )
        })?;
    if bytes.len() as u64 > MAX_DUMP_META_BYTES {
        return Err(AmError::new(
            ErrorCode::ArchiveCorrupt,
            format!(
                "the staged {DUMP_META_MEMBER} holds {} bytes or more, over the limit this \
                 build reads",
                bytes.len()
            ),
        ));
    }
    Ok(bytes)
}

/// Validate one staged `dump/meta.json` file.
///
/// This is the payload check a verify pass runs for a Postgres archive: the
/// payload member is the dump meta, so "is a real payload" means "parses and
/// passes its own validation". The file is read once, bounded, and never
/// interpreted beyond the meta's own rules.
///
/// # Errors
/// The file is unreadable, over the size bound, or fails its validation.
pub fn validate_dump_meta_file(path: &Path) -> Result<(), AmError> {
    let bytes = read_bounded_meta_bytes(path)?;
    DumpMeta::from_json_bytes(&bytes).map(|_| ())
}

#[cfg(test)]
#[path = "dump_meta_tests.rs"]
mod tests;
