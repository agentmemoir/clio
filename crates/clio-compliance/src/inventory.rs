// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the read-only compliance scope ledger: which
//! stored family can be selected for one data subject, where each family's
//! source and subject link lives, and which rows stay open because no such
//! link exists.
//!
//! ## Owns
//! - `scope_inventory`, the builder over live store facts and resolved paths
//! - The ledger vocabulary: rows, owners, custody, readability, actions
//! - Reconciling the live store's tables against the rows that name them
//!
//! ## Does not own
//! - Reading the store: the caller passes `clio_store` measurement facts in
//! - Resolving paths and rendering the report (the command surface)
//! - Any write: rows are never removed, and declared copy paths are only read
//!   with `std::fs::metadata`
//!
//! ## Boundary
//!
//! Changes to what the ledger claims about a stored family should normally
//! require changes here. Changes to how a store is measured should not.
//!
//! Keep this module focused on the read-only scope ledger.

#[path = "inventory_rows.rs"]
pub(crate) mod inventory_rows;

use self::inventory_rows::{ROW_PLAN, RowPlan, missing_tables, named_by_plan, pg_source_note};
use clio_store::{PostgresTargetState, SqliteFileState};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Which store backend a ledger describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryBackend {
    /// A SQLite store file.
    Sqlite,
    /// A Postgres target schema.
    Postgres,
}

/// Who owns a row's retention decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Owner {
    /// The Clio store holds and owns the row.
    Clio,
    /// An operator holds it outside the live store.
    Operator,
    /// A caller holds it outside the live store.
    Caller,
    /// A peer node holds it.
    Peer,
    /// No holder is known.
    Unknown,
}

/// Who holds a row's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Custody {
    /// The live Clio store holds it.
    Clio,
    /// An operator-held location (archives, sidecars) holds it.
    Operator,
    /// A caller-held copy holds it.
    Caller,
    /// A peer node holds it.
    Peer,
    /// No holder is known.
    Unknown,
}

/// How a row's stored form can be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Readability {
    /// Sealed content: unreadable without the protecting key.
    Sealed,
    /// Plaintext columns at rest.
    Plaintext,
    /// A one-way hash only.
    Hashed,
    /// Ids, counts, and timestamps only, never content.
    Metadata,
    /// Key material or a DEK reference.
    KeyMaterial,
    /// Not established.
    Unknown,
}

/// The action the ledger records for a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionAction {
    /// Subject-scoped crypto-shredding and derived-structure purge.
    Erase,
    /// Rebuildable derived state: wipe and rebuild from the store's own rows.
    Regenerate,
    /// Operations removal of dead rows or graph debris, never fact invalidation.
    Prune,
    /// Keep as it is: history or provenance that must survive.
    Retain,
    /// Record what the holder says; Clio never deletes a copy it does not hold.
    Record,
    /// The action is undecided because the record or the verifier does not exist.
    Open,
}

/// How a row's source description is resolved.
pub(crate) enum RowSource {
    /// The row's named store tables, joined.
    Tables,
    /// A fixed description (a record that is stored nowhere).
    Fixed(&'static str),
    /// The store file, or the Postgres target schema.
    StoreFile,
    /// The per-store key file.
    KmsFile,
    /// The backup archives root.
    BackupDir,
    /// The declared caller copies, one row per declared path.
    Copies,
    /// The enabled-peer set and its acknowledgements.
    Peers,
}

/// One row of the scope ledger: one stored family or auxiliary record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InventoryRow {
    /// Stable family name, the same on every backend.
    pub family: &'static str,
    /// The resolved source path or table description for this backend.
    pub source: String,
    /// The subject link that selects the family, when one exists.
    pub subject_link: Option<String>,
    /// The named gap when the link is absent, hashed, legacy, or unknown.
    pub gap: Option<String>,
    /// Who owns the retention decision.
    pub owner: Owner,
    /// Who holds the row's content.
    pub custody: Custody,
    /// How the stored form can be read.
    pub readability: Readability,
    /// The key material protecting the row, when any.
    pub key_path: Option<String>,
    /// What the ledger says should happen to the row.
    pub retention_action: RetentionAction,
    /// The test function or repository source that proves the row.
    pub proof: &'static str,
    /// Whether any named reason applies to this row.
    pub open: bool,
    /// The named reasons, present iff `open`.
    pub open_reason: Option<String>,
}

/// The live-store facts a ledger run is built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InventoryFacts {
    /// Facts measured read-only from a SQLite store file.
    Sqlite(SqliteFileState),
    /// Facts measured read-only from a Postgres target.
    Postgres(PostgresTargetState),
}

impl InventoryFacts {
    /// Every measured table with its row count, ordered by table name.
    fn tables(&self) -> &[(String, u64)] {
        match self {
            Self::Sqlite(state) => &state.table_counts,
            Self::Postgres(state) => &state.table_counts,
        }
    }

    /// Why the store itself is missing, when it is.
    fn store_missing_reason(&self) -> Option<String> {
        match self {
            Self::Sqlite(state) if !state.exists => {
                Some(format!("store file not found at {}", state.path.display()))
            }
            Self::Postgres(state) if state.table_counts.is_empty() => {
                Some("postgres target holds no measured application tables".to_owned())
            }
            _ => None,
        }
    }
}

/// The resolved local paths a ledger run describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryPaths {
    /// The SQLite store file, or a placeholder for a Postgres target.
    pub db_path: PathBuf,
    /// The per-store key file.
    pub kms_path: PathBuf,
    /// The backup archives root.
    pub backup_dir: PathBuf,
}

/// One ledger run's inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryInput {
    /// The live-store facts the rows are decided against.
    pub facts: InventoryFacts,
    /// The resolved local paths.
    pub paths: InventoryPaths,
    /// Operator- or recipient-held copies the caller declared.
    pub declared_copies: Vec<PathBuf>,
    /// Whether enabled peers exist; `None` means that could not be verified.
    pub peers_enabled: Option<bool>,
}

/// The scope ledger for one store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScopeInventory {
    /// Which backend these rows describe.
    pub backend: InventoryBackend,
    /// One row per family and auxiliary record; rows are never removed.
    pub rows: Vec<InventoryRow>,
    /// Live store tables that no row names, in table-name order.
    pub unclassified: Vec<String>,
    /// True only when every row is closed and nothing is unclassified.
    pub complete: bool,
}

/// Build the read-only scope ledger for one store.
///
/// A row is open when its pinned source is missing from the live facts, when
/// its link is absent or hashed or legacy, when a declared copy's custody is
/// unknown, when a required local path is missing, or when enabled peers
/// cannot be verified. Nothing is written, no row is removed, and a declared
/// copy path is only ever read with `std::fs::metadata`.
#[must_use]
pub fn scope_inventory(input: &InventoryInput) -> ScopeInventory {
    let backend = match &input.facts {
        InventoryFacts::Sqlite(_) => InventoryBackend::Sqlite,
        InventoryFacts::Postgres(_) => InventoryBackend::Postgres,
    };
    let mut rows = Vec::with_capacity(ROW_PLAN.len() + input.declared_copies.len());
    for plan in ROW_PLAN {
        if matches!(plan.source, RowSource::Copies) {
            rows.extend(copy_rows(plan, input));
            continue;
        }
        rows.push(planned_row(plan, input));
    }
    let unclassified: Vec<String> = input
        .facts
        .tables()
        .iter()
        .filter(|(name, _)| !named_by_plan(name))
        .map(|(name, _)| name.clone())
        .collect();
    let complete = unclassified.is_empty() && rows.iter().all(|row| !row.open);
    ScopeInventory {
        backend,
        rows,
        unclassified,
        complete,
    }
}

/// One planned row, with every open reason the live facts produced.
fn planned_row(plan: &RowPlan, input: &InventoryInput) -> InventoryRow {
    let mut reasons = missing_source_reasons(plan, input);
    if let Some(gap) = plan.gap {
        reasons.push(gap.to_owned());
    }
    path_reasons(plan, input, &mut reasons);
    peer_reasons(plan, input, &mut reasons);
    row_from_parts(
        plan,
        resolve_source(plan, input),
        plan.gap.map(str::to_owned),
        plan.custody,
        &reasons,
    )
}

/// One declared caller copy per path; with none declared, the single row that
/// records that copies held outside the store stay unknown.
fn copy_rows(plan: &RowPlan, input: &InventoryInput) -> Vec<InventoryRow> {
    if input.declared_copies.is_empty() {
        return vec![planned_row(plan, input)];
    }
    input
        .declared_copies
        .iter()
        .map(|path| {
            let gap =
                "held by the caller outside the store: only the declared path is known".to_owned();
            let reason = if present(path) {
                "declared caller copy: Clio records it and never deletes it".to_owned()
            } else {
                format!("declared copy not found at {}", path.display())
            };
            row_from_parts(
                plan,
                format!("declared caller copy at {}", path.display()),
                Some(gap),
                Custody::Caller,
                &[reason],
            )
        })
        .collect()
}

/// One row assembled from a plan plus the values this run resolved.
fn row_from_parts(
    plan: &RowPlan,
    source: String,
    gap: Option<String>,
    custody: Custody,
    reasons: &[String],
) -> InventoryRow {
    let open = !reasons.is_empty();
    InventoryRow {
        family: plan.family,
        source,
        subject_link: plan.link.map(str::to_owned),
        gap,
        owner: plan.owner,
        custody,
        readability: plan.readability,
        key_path: plan.key_path.map(str::to_owned),
        retention_action: plan.action,
        proof: plan.proof,
        open,
        open_reason: open.then(|| reasons.join("; ")),
    }
}

/// The planned tables the live facts do not hold, as one named reason, plus
/// the named per-backend handling of a source this backend cannot resolve.
fn missing_source_reasons(plan: &RowPlan, input: &InventoryInput) -> Vec<String> {
    let postgres = matches!(input.facts, InventoryFacts::Postgres(_));
    let missing: Vec<&str> = missing_tables(plan, input.facts.tables())
        .into_iter()
        .filter(|pattern| !postgres || pg_source_note(pattern).is_none())
        .collect();
    let mut reasons = Vec::new();
    if !missing.is_empty() {
        reasons.push(format!(
            "pinned source not in the live store facts: {}",
            missing.join(", ")
        ));
    }
    if postgres {
        for pattern in plan.tables {
            reasons.extend(pg_source_note(pattern).map(str::to_owned));
        }
    }
    reasons
}

/// The reasons a required local path is missing.
fn path_reasons(plan: &RowPlan, input: &InventoryInput, reasons: &mut Vec<String>) {
    match plan.source {
        RowSource::StoreFile => {
            if let Some(reason) = input.facts.store_missing_reason() {
                reasons.push(reason);
            }
        }
        RowSource::KmsFile if !present(&input.paths.kms_path) => {
            reasons.push(format!(
                "key file not found at {}",
                input.paths.kms_path.display()
            ));
        }
        RowSource::BackupDir if !present(&input.paths.backup_dir) => {
            reasons.push(format!(
                "backup archives root not found at {}",
                input.paths.backup_dir.display()
            ));
        }
        _ => {}
    }
}

/// The reason enabled peers could not be verified, when they could not.
fn peer_reasons(plan: &RowPlan, input: &InventoryInput, reasons: &mut Vec<String>) {
    if !matches!(plan.source, RowSource::Peers) {
        return;
    }
    match input.peers_enabled {
        None => {
            reasons.push("peers could not be verified: no enablement fact was supplied".to_owned());
        }
        Some(true) => reasons.push(
            "peers are enabled and no acknowledgement record exists to verify them".to_owned(),
        ),
        Some(false) => {}
    }
}

/// Whether `path` exists, through the ledger's only filesystem access: a
/// read-only `std::fs::metadata` call. Nothing is created, opened, or removed.
fn present(path: &Path) -> bool {
    std::fs::metadata(path).is_ok()
}

/// The `-wal`/`-shm` sidecar state beside a SQLite store file, named through
/// the same read-only `std::fs::metadata` check as every other local path.
fn sidecar_trace(db_path: &Path) -> String {
    let trace = |suffix: &str| {
        let mut sidecar = db_path.as_os_str().to_owned();
        sidecar.push(suffix);
        let state = if present(Path::new(&sidecar)) {
            "present"
        } else {
            "absent"
        };
        format!("{suffix} {state}")
    };
    format!("{}, {}", trace("-wal"), trace("-shm"))
}

/// The resolved source description of one row.
fn resolve_source(plan: &RowPlan, input: &InventoryInput) -> String {
    match plan.source {
        RowSource::Tables => plan.tables.join(", "),
        RowSource::Fixed(text) => text.to_owned(),
        RowSource::StoreFile => match &input.facts {
            InventoryFacts::Sqlite(state) => format!(
                "store file {} (sidecars: {})",
                state.path.display(),
                sidecar_trace(&state.path)
            ),
            InventoryFacts::Postgres(state) => {
                format!("postgres target schema {}", state.schema)
            }
        },
        RowSource::KmsFile => format!("key file {}", input.paths.kms_path.display()),
        RowSource::BackupDir => {
            format!("backup archives under {}", input.paths.backup_dir.display())
        }
        RowSource::Copies => "no caller copy declared".to_owned(),
        RowSource::Peers => "peer enablement and acknowledgements (not stored)".to_owned(),
    }
}

#[cfg(test)]
#[path = "inventory_tests.rs"]
mod tests;
