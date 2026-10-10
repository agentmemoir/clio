// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for Postgres app-table discovery: reading the
//! catalog for one schema's relations, their columns, and the facts a backup
//! half needs to treat them soundly (extension ownership and dependency,
//! owned sequences, and the installed extensions).
//!
//! ## Owns
//! - `effective_schema`: the schema the connection's search path resolves to
//! - `discover_schema`: the one-query relation and column enumeration
//! - `DiscoveredSchema` and its three relation classes: app tables,
//!   extension-dependent tables, and extension-owned tables
//! - `OwnedSequence`, the forward-compatibility finding a snapshot refuses on
//!
//! ## Does not own
//! - The dump and restore transactions (see `postgres_dump`, `postgres_restore`)
//! - The pure table shapes and member naming (see `postgres_backup_model`)
//! - Store open and migration (see `postgres`, which a snapshot deliberately
//!   does not run)
//!
//! ## Boundary
//!
//! Changes to which relations count as application tables should normally
//! require changes here. Changes to how the discovered tables are copied or
//! restored should not require changes here.
//!
//! Keep this module focused on catalog discovery.

use crate::migrate::IntoAmError;
use crate::postgres_backup_model::{PgColumn, PgTable};
use clio_types::{AmError, ErrorCode};
use postgres::Transaction;

/// One owned sequence: the sequence, the table and column whose next value it
/// supplies.
///
/// A restore cannot reproduce a sequence's position, so a schema with one is
/// one a logical snapshot refuses to take rather than one it silently makes
/// unrestoreable. `serial` and `GENERATED ... AS IDENTITY` both create one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedSequence {
    /// The sequence's name, unquoted.
    pub sequence: String,
    /// The table the sequence supplies.
    pub table: String,
    /// The column the sequence supplies.
    pub column: String,
}

/// What one schema's catalog says, sorted and classified.
///
/// `app_tables` is the set a dump copies: base tables with no extension
/// dependency, sorted by name, columns in ordinal order. `extension_dependent`
/// is the set a restore clears but never copies — derived tables whose rows
/// the store rebuilds (the vector table today). `extension_owned` is the set
/// nothing in this phase touches. Discovery filters by dependency rather than
/// by a hardcoded list, so a future migration's new table is picked up without
/// a code change here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredSchema {
    /// The schema that was discovered.
    pub schema: String,
    /// The base tables a dump copies, sorted by table name.
    pub app_tables: Vec<PgTable>,
    /// Base tables that depend on an extension (for example through a
    /// `vector` column), sorted by name. Cleared at restore, never dumped.
    pub extension_dependent: Vec<String>,
    /// Base tables owned by an extension, sorted by name. Never touched.
    pub extension_owned: Vec<String>,
    /// Every owned sequence in the schema, sorted by sequence name.
    pub sequences: Vec<OwnedSequence>,
    /// The installed extensions, sorted by name.
    pub extensions: Vec<String>,
}

impl DiscoveredSchema {}

/// The schema the connection's `search_path` resolves to first.
///
/// This is the store's effective schema: the test harness scopes it through
/// the connection options, and production resolves it the same way, so
/// discovery never assumes `public`.
///
/// # Errors
/// The `current_schema()` read fails, or the search path resolves to nothing.
pub fn effective_schema(tx: &mut Transaction) -> Result<String, AmError> {
    let row = tx
        .query_opt("SELECT pg_catalog.current_schema()", &[])
        .am("read the effective schema")?;
    let name = row.and_then(|row| row.get::<_, Option<String>>(0));
    name.ok_or_else(|| {
        AmError::new(
            ErrorCode::ConfigCorrupt,
            "the connection's search_path resolves to no schema, so there is no effective \
             schema to discover",
        )
    })
}

/// Discover `schema`'s relations in one catalog query.
///
/// The query enumerates every relation of the relation kinds a store can hold
/// with its columns in ordinal order, and classifies each relation in the same
/// pass: extension-owned (the relation itself belongs to an extension) and
/// extension-dependent (a column or default depends on an extension-owned
/// object). Two small catalog reads follow it: the owned sequences and the
/// installed extensions. No query runs per table.
///
/// Runs inside whatever transaction the caller holds, so a dump's discovery
/// reads the same snapshot its copies read.
///
/// # Errors
/// Any catalog read fails.
pub fn discover_schema(tx: &mut Transaction, schema: &str) -> Result<DiscoveredSchema, AmError> {
    let mut relations = RelationSet::default();
    let rows = tx
        .query(RELATION_SQL, &[&schema])
        .am("discover the schema's relations")?;
    for row in rows {
        relations.absorb(&row);
    }
    let sequences = read_sequences(tx, schema)?;
    let extensions = read_extensions(tx)?;
    Ok(DiscoveredSchema {
        schema: schema.to_owned(),
        app_tables: relations.app_tables(),
        extension_dependent: relations.extension_dependent(),
        extension_owned: relations.extension_owned(),
        sequences,
        extensions,
    })
}

/// The relation-and-columns catalog query, one round trip for the whole
/// schema.
///
/// `relkind` `r` and `p` are base tables (ordinary and partitioned); `v`, `m`,
/// and `f` are views, materialized views, and foreign tables, which are
/// enumerated only to be classified out. Generated columns (`GENERATED ...
/// AS ... STORED`) are dropped from the column list: a `COPY ... FROM STDIN`
/// cannot write one, so listing it would make every load fail.
const RELATION_SQL: &str = "
SELECT c.relname,
       c.relkind::text,
       a.attnum,
       a.attname,
       pg_catalog.format_type(a.atttypid, a.atttypmod) AS coltype,
       EXISTS (
         SELECT 1 FROM pg_catalog.pg_depend d
         JOIN pg_catalog.pg_depend e
           ON e.classid = d.refclassid AND e.objid = d.refobjid
          AND e.objsubid = 0 AND e.deptype = 'e'
         WHERE d.objid = c.oid AND d.refobjid <> c.oid
           AND d.deptype IN ('n', 'a')
       ) AS extension_dependent,
       EXISTS (
         SELECT 1 FROM pg_catalog.pg_depend d
         WHERE d.objid = c.oid AND d.classid = 'pg_catalog.pg_class'::regclass
           AND d.deptype = 'e'
       ) AS extension_owned
FROM pg_catalog.pg_class c
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
LEFT JOIN pg_catalog.pg_attribute a
       ON a.attrelid = c.oid AND a.attnum > 0
      AND NOT a.attisdropped AND a.attgenerated = ''
WHERE n.nspname = $1 AND c.relkind IN ('r', 'p', 'v', 'm', 'f')
ORDER BY c.relname, a.attnum";

/// The owned-sequence catalog read: every sequence in the schema that supplies
/// a table column, with the table and column it supplies.
const SEQUENCE_SQL: &str = "
SELECT s.relname, t.relname, a.attname
FROM pg_catalog.pg_class s
JOIN pg_catalog.pg_namespace n ON n.oid = s.relnamespace
JOIN pg_catalog.pg_depend d ON d.objid = s.oid AND d.deptype = 'a'
JOIN pg_catalog.pg_class t ON t.oid = d.refobjid
JOIN pg_catalog.pg_attribute a ON a.attrelid = t.oid AND a.attnum = d.refobjsubid
WHERE n.nspname = $1 AND s.relkind = 'S'
ORDER BY s.relname";

/// The installed-extension catalog read.
const EXTENSION_SQL: &str = "SELECT extname FROM pg_catalog.pg_extension ORDER BY extname";

/// The relations one catalog query produced, before classification.
///
/// The query returns one row per (relation, column) pair and one row per
/// column-less relation, so this collects the column rows into per-relation
/// column lists and keeps the classification flags per relation.
#[derive(Default)]
struct RelationSet {
    tables: Vec<PgRelation>,
}

/// One relation from the catalog query, with the column rows folded in.
#[derive(Default)]
struct PgRelation {
    name: String,
    is_base_table: bool,
    extension_dependent: bool,
    extension_owned: bool,
    columns: Vec<PgColumn>,
}

impl RelationSet {
    /// Fold one catalog row in. Rows of one relation arrive in ordinal order,
    /// so the column list is built in order without a sort.
    fn absorb(&mut self, row: &postgres::Row) {
        let name: String = row.get("relname");
        let relkind: String = row.get("relkind");
        let extension_dependent: bool = row.get("extension_dependent");
        let extension_owned: bool = row.get("extension_owned");
        if self.tables.last().is_none_or(|last| last.name != name) {
            self.tables.push(PgRelation {
                name: name.clone(),
                is_base_table: matches!(relkind.as_str(), "r" | "p"),
                extension_dependent,
                extension_owned,
                columns: Vec::new(),
            });
        }
        let ordinal: Option<i32> = row
            .try_get::<_, Option<i16>>("attnum")
            .ok()
            .flatten()
            .map(i32::from);
        let column_name: Option<String> = row.try_get("attname").ok().flatten();
        if let (Some(ordinal), Some(column_name)) = (ordinal, column_name) {
            let type_name: Option<String> = row.try_get("coltype").ok().flatten();
            let relation = self
                .tables
                .last_mut()
                .expect("the relation was just pushed");
            relation.columns.push(PgColumn {
                name: column_name,
                ordinal,
                type_name: type_name.unwrap_or_default(),
            });
        }
    }

    /// The app tables: base tables that neither belong to nor depend on an
    /// extension, sorted by name (the query's order).
    fn app_tables(&self) -> Vec<PgTable> {
        self.tables
            .iter()
            .filter(|t| t.is_base_table && !t.extension_dependent && !t.extension_owned)
            .map(|t| PgTable {
                name: t.name.clone(),
                columns: t.columns.clone(),
            })
            .collect()
    }

    /// The extension-dependent base tables, sorted by name.
    fn extension_dependent(&self) -> Vec<String> {
        self.tables
            .iter()
            .filter(|t| t.is_base_table && t.extension_dependent && !t.extension_owned)
            .map(|t| t.name.clone())
            .collect()
    }

    /// The extension-owned relations, sorted by name.
    fn extension_owned(&self) -> Vec<String> {
        self.tables
            .iter()
            .filter(|t| t.extension_owned)
            .map(|t| t.name.clone())
            .collect()
    }
}

/// Read the schema's owned sequences.
fn read_sequences(tx: &mut Transaction, schema: &str) -> Result<Vec<OwnedSequence>, AmError> {
    let rows = tx
        .query(SEQUENCE_SQL, &[&schema])
        .am("read owned sequences")?;
    Ok(rows
        .iter()
        .map(|row| OwnedSequence {
            sequence: row.get(0),
            table: row.get(1),
            column: row.get(2),
        })
        .collect())
}

/// Read the installed extensions, sorted by name.
fn read_extensions(tx: &mut Transaction) -> Result<Vec<String>, AmError> {
    let rows = tx
        .query(EXTENSION_SQL, &[])
        .am("read installed extensions")?;
    Ok(rows.iter().map(|row| row.get::<_, String>(0)).collect())
}

#[cfg(test)]
#[path = "postgres_discovery_tests.rs"]
mod tests;
