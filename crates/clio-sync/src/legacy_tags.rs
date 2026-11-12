// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for reading the legacy v1 domain tags that older
//! builds hashed into saved sync values, so verification can still accept a
//! value written before the typed-domain change.
//!
//! ## Owns
//! - Parsing the checked-in tag data file
//! - Looking one tag up by name
//!
//! ## Does not own
//! - The v2 domain tags (see `clio_store::erase_hash`)
//! - Which sync value uses which tag (`keyed_roles`, `protocol`)
//!
//! ## Boundary
//!
//! Changes to the legacy tag set should normally require changes here and in
//! the data file. Changes to how a v2 digest is built should not.

use std::sync::OnceLock;

/// The checked-in legacy tag data: one `name<TAB>value` row per tag.
const LEGACY_TAGS_TSV: &str = include_str!("../assets/legacy_domain_tags.tsv");

/// Row name of the plaintext-mode content digest tag.
pub(crate) const PLAINTEXT_DIGEST: &str = "plaintext_digest";

/// Row name of the plaintext-mode mutation idempotency tag.
pub(crate) const PLAINTEXT_MUTATION_ID: &str = "plaintext_mutation_id";

/// Row name of the sync cursor checksum tag.
pub(crate) const SYNC_CURSOR: &str = "sync_cursor";

/// Row name of the sync watermark checksum tag.
pub(crate) const SYNC_WATERMARK: &str = "sync_watermark";

type Rows = Vec<(&'static str, &'static str)>;

static ROWS: OnceLock<Result<Rows, String>> = OnceLock::new();

/// Parse the data file exactly once.
fn parse(tsv: &'static str) -> Result<Rows, String> {
    let mut rows: Rows = Vec::new();
    for (index, line) in tsv.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('\t') else {
            return Err(format!(
                "legacy domain tags: line {} has no tab separator",
                index + 1
            ));
        };
        if name.is_empty() || value.is_empty() {
            return Err(format!(
                "legacy domain tags: line {} has an empty field",
                index + 1
            ));
        }
        rows.push((name, value));
    }
    if rows.is_empty() {
        return Err("legacy domain tags: the data file has no rows".to_owned());
    }
    Ok(rows)
}

/// The parsed rows, or a clear error when the data file is malformed.
fn rows() -> Result<&'static Rows, String> {
    match ROWS.get_or_init(|| parse(LEGACY_TAGS_TSV)) {
        Ok(rows) => Ok(rows),
        Err(err) => Err(err.clone()),
    }
}

/// The tag bytes stored under `name`.
///
/// # Errors
/// The data file is malformed, or it has no row with that name.
pub(crate) fn legacy_tag(name: &str) -> Result<&'static str, String> {
    for (row_name, value) in rows()? {
        if *row_name == name {
            return Ok(value);
        }
    }
    Err(format!("legacy domain tags: no row named `{name}`"))
}

/// The v1 hex digest of `value` under the tag stored as `name`.
///
/// # Errors
/// The data file is malformed, or it has no row with that name.
pub(crate) fn legacy_sha256_hex(name: &str, value: &str) -> Result<String, String> {
    Ok(clio_store::sha256_hex(legacy_tag(name)?.as_bytes(), value))
}

#[cfg(test)]
#[path = "legacy_tags_tests.rs"]
mod tests;
