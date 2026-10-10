// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for how one created snapshot is shown: the facts
//! a script parses and the lines an operator reads.
//!
//! ## Owns
//! - `payload`, the JSON report of what was written and what the archive holds
//! - `render`, the human view of the same facts
//! - The `keys` warning an archive holding key material must carry
//!
//! ## Does not own
//! - The `create` flags, the snapshot itself, and the exit codes (see
//!   `backup_create_cli`)
//! - Archive assembly and the manifest (see `clio-backup`'s `create`)
//!
//! ## Boundary
//!
//! Changes to how a create is worded or shaped should normally require changes
//! here. Changes to the order a create runs in should not require changes here.
//!
//! Keep this module focused on rendering one created snapshot.

use crate::backup_create_cli::SnapshotView;
use clio_backup::CreateReport;
use serde_json::{Value, json};

/// What the report says when the archive holds key material.
///
/// The manifest note and the `--help` text say the same thing; the report is the
/// third place an operator reads, so it must not be the one that stays quiet.
const KEY_MATERIAL_WARNING: &str = "this archive holds key material; keep it owner-only";

/// The `create` report: what was written, and what it holds.
pub(super) fn payload(archive: &CreateReport, facts: &SnapshotView) -> Value {
    json!({
        "ok": true,
        "snapshot_id": archive.id,
        "path": archive.path.display().to_string(),
        "backend": facts.backend,
        "schema": facts.schema,
        "database": facts.database,
        "payload_bytes": archive.payload_bytes,
        "archive_bytes": archive.archive_bytes,
        "uncompressed_bytes": archive.uncompressed_bytes,
        "compression_ratio": archive.compression_ratio,
        "duration_ms": facts.duration_ms,
        "counts": counts(facts),
        "keys": keys(archive),
        "schema_version": facts.schema_version,
    })
}

/// The `keys` object: what the archive carries, plus the warning it must carry
/// with it.
///
/// The warning is a field inside `keys`, not a new top-level key, so a script
/// reading the report's shape is unaffected. It appears only when a key member
/// is carried: an archive without one holds no key material, so there is
/// nothing to warn about.
fn keys(archive: &CreateReport) -> Value {
    let mut keys = serde_json::Map::new();
    keys.insert("included".to_owned(), json!(archive.keys_included));
    keys.insert("subjects".to_owned(), json!(archive.keys_subjects));
    if archive.keys_included {
        keys.insert("warning".to_owned(), json!(KEY_MATERIAL_WARNING));
    }
    Value::Object(keys)
}

/// The counted rows: the headline numbers plus the full per-table map.
fn counts(facts: &SnapshotView) -> Value {
    let mut by_table = serde_json::Map::new();
    for (table, count) in &facts.table_counts {
        by_table.insert(table.clone(), json!(count));
    }
    json!({
        "complete": true,
        "tables": facts.table_count(),
        "items": facts.count_of("items").unwrap_or(0),
        "triples": facts.count_of("triples").unwrap_or(0),
        "by_table": by_table,
    })
}

/// The human view: the id, the path, and what the archive holds.
pub(super) fn render(archive: &CreateReport, facts: &SnapshotView) -> String {
    let keys = if archive.keys_included {
        format!("included ({} subjects)", archive.keys_subjects)
    } else {
        "not included".to_owned()
    };
    let schema = facts.schema_version.as_deref().unwrap_or("unknown");
    let store_schema = facts
        .schema
        .as_deref()
        .map(|schema| format!(" · store schema {schema}"))
        .unwrap_or_default();
    let mut lines = vec![format!(
        "snapshot {} -> {}\n  payload {} B · archive {} B · ratio {}x · {} ms\n  counts {} \
         tables (items {}, triples {})\n  backend {}{store_schema} · keys {keys} · schema \
         {schema}",
        archive.id,
        archive.path.display(),
        archive.payload_bytes,
        archive.archive_bytes,
        archive.compression_ratio,
        facts.duration_ms,
        facts.table_count(),
        facts.count_of("items").unwrap_or(0),
        facts.count_of("triples").unwrap_or(0),
        facts.backend,
    )];
    // The archive holds key material, so the report says so in the same words
    // the manifest note uses. Nothing is printed when no key member is carried.
    if archive.keys_included {
        lines.push(format!("  warning: {KEY_MATERIAL_WARNING}"));
    }
    lines.join("\n")
}
