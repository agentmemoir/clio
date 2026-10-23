// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for reading the dump meta of a Postgres archive
//! and for turning the subject columns of its dump tables into membership facts.
//!
//! ## Owns
//! - `read_dump_meta`, `table_named`, and `classify_dump_table`
//!
//! ## Does not own
//! - The archive's classification as a whole (`restore_authority` owns that)
//! - The COPY text reader (`restore_authority_copy` owns that)
//! - The SQLite payload and key-store reads (`restore_authority_sqlite` owns
//!   those)
//!
//! ## Boundary
//!
//! Changes to how a Postgres dump table names its subjects should normally
//! require changes here. Changes to the authority rules should not.
//!
//! Keep this module focused on the Postgres side of one archive.

use super::{
    ClassifyRefusal, ITEMS_TABLE, SUBJECT_ID_COLUMN, SUBJECT_KEYS_TABLE, TOMBSTONE_HASH_COLUMN,
    TOMBSTONE_TABLE, facts_from_hashes, io_error,
};
use clio_backup::ExtractedArchive;
use clio_compliance::{MemberFacts, tombstone_salt};
use clio_store::sha256_hex;
use std::collections::BTreeSet;
use std::path::Path;

/// Parse the staged payload as a Postgres dump meta.
pub(super) fn read_dump_meta(
    payload: &Path,
) -> Result<clio_backup::dump_meta::DumpMeta, ClassifyRefusal> {
    let bytes = std::fs::read(payload)
        .map_err(|error| io_error("reading the dump meta", payload, &error))?;
    Ok(clio_backup::dump_meta::DumpMeta::from_json_bytes(&bytes)?)
}

/// The dump table the dump meta declares under `member`, if any.
pub(super) fn table_named<'a>(
    meta: &'a clio_backup::dump_meta::DumpMeta,
    member: &str,
) -> Option<&'a clio_backup::dump_meta::DumpTable> {
    meta.tables.iter().find(|table| table.member == member)
}

/// Classify one Postgres dump table member from its `COPY` text.
///
/// Only the three subject tables carry a subject reference; every other table
/// is content and proves no content-free subject record. The item table is one
/// of them: an item row keeps its subject link after the subject's key was
/// destroyed, so the archive names the subject. A subject table whose subject
/// column is absent cannot be classified, so the archive is refused rather than
/// passing one whose membership is unknown.
pub(super) fn classify_dump_table(
    table: &clio_backup::dump_meta::DumpTable,
    extracted: &ExtractedArchive,
    live: &BTreeSet<String>,
    keys_present: bool,
) -> Result<Vec<MemberFacts>, ClassifyRefusal> {
    let column = match table.table.as_str() {
        TOMBSTONE_TABLE => Some((TOMBSTONE_HASH_COLUMN, false)),
        SUBJECT_KEYS_TABLE | ITEMS_TABLE => Some((SUBJECT_ID_COLUMN, true)),
        _ => None,
    };
    let Some((column, hash_the_id)) = column else {
        return Ok(vec![MemberFacts::None]);
    };
    let Some(index) = table.columns.iter().position(|entry| entry.name == column) else {
        return Err(ClassifyRefusal::UnknownMember(table.member.clone()));
    };
    let Some(member) = extracted
        .dump_tables
        .iter()
        .find(|member| member.member == table.member)
    else {
        return Err(ClassifyRefusal::UnknownMember(table.member.clone()));
    };
    let file = member.file.open()?;
    let salt = tombstone_salt();
    let mut hashes = BTreeSet::new();
    for value in super::restore_authority_copy::copy_column(std::io::BufReader::new(file), index)? {
        if hash_the_id {
            hashes.insert(sha256_hex(&salt, &value));
        } else {
            hashes.insert(value);
        }
    }
    Ok(facts_from_hashes(hashes, live, keys_present))
}

#[cfg(test)]
#[path = "restore_authority_dump_tests.rs"]
mod restore_authority_dump_tests;
