// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared synthetic fixtures of the
//! cross-path compliance proof: the one bank, the two subjects with their
//! distinct content markers, the per-test scratch directory, the seeded
//! SQLite and Postgres stores, and the journal and backup paths a scenario
//! drives the real `clio` binary against.
//!
//! ## Owns
//! - The synthetic bank, subjects, item ids, and content markers
//! - `unique`, `scratch`, `item`, `write_item`, `open_sqlite`, `open_pg`,
//!   `seed_sqlite`, and `seed_pg`
//! - `journal_file` and `backup_dir`, the CLI-visible paths
//!
//! ## Does not own
//! - Archive assembly and the read-back probes (see `support`)
//! - The Postgres gate and dump restore (see `support_pg`)
//! - The CLI driver (see `cli`)
//!
//! ## Boundary
//!
//! Changes to how a synthetic subject is named or seeded should normally require
//! changes here. Changes to a scenario's assertions should not.
//!
//! Keep this module focused on fixtures, not on scenario logic or measurement.

use crate::common;
use clio::store::{DekProvider, LocalDevKms, PostgresStore, SqliteStore, Store};
use clio::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The one bank every scenario seeds and reads.
pub const BANK: &str = "cross-path-bank";

/// The subject every erasure and denial names.
pub const DENIED_SUBJECT: &str = "cross-path-denied-subject";

/// The subject no erasure touches: the readability control.
pub const CONTROL_SUBJECT: &str = "cross-path-control-subject";

/// The denied subject's only item.
pub const DENIED_ITEM: &str = "cross-path-denied-item";

/// The control subject's only item.
pub const CONTROL_ITEM: &str = "cross-path-control-item";

/// The content marker only the denied subject's item carries, so a read-back can
/// tell the denied content apart from the control content.
pub const DENIED_MARKER: &str = "marker-denied-cross-path";

/// The content marker only the control subject's item carries.
pub const CONTROL_MARKER: &str = "marker-control-cross-path";

/// The fixed transaction stamp every fixture uses; never a real clock.
pub const NOW: &str = "2026-01-01T00:00:00Z";

/// A unique string per call, so two tests never share a path or a name.
pub fn unique(tag: &str) -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("cross-path-{tag}-{}-{sequence}", std::process::id())
}

/// A fresh, unique scratch directory for one scenario, removed on drop.
///
/// The directory is created under the system temp root and adopted through the
/// shared harness guard, so a finished scenario leaves nothing behind.
pub fn scratch(tag: &str) -> common::Scratch {
    let path = std::env::temp_dir().join(unique(tag));
    std::fs::create_dir_all(&path).expect("create the scratch directory");
    common::Scratch::adopt(path)
}

/// One synthetic item carrying `subject` and `gist`, sealed on write.
fn item(id: &str, subject: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.to_owned(),
        bank_id: BANK.to_owned(),
        subject_id: subject.to_owned(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot: Some(json!({"entity": "synthetic cross-path subject"})),
        gist: Some(gist.to_owned()),
        valid_from: NOW.into(),
        valid_until: None,
        tx_from: NOW.into(),
        tx_until: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        context: None,
    }
}

/// Write one synthetic item into an open store.
pub fn write_item<S: Store>(store: &S, id: &str, subject: &str, gist: &str) {
    store
        .create_memory_item(&item(id, subject, gist), "harness")
        .expect("write the synthetic item");
}

/// Open a SQLite store backed by the file key store at `kms_path`.
pub fn open_sqlite(db: &Path, kms_path: &Path) -> SqliteStore {
    let kms: Arc<dyn DekProvider> =
        Arc::new(LocalDevKms::open_persistent(kms_path).expect("open the file key store"));
    SqliteStore::open(db, kms).expect("open the sqlite store")
}

/// Open a Postgres store backed by the file key store at `kms_path`.
pub fn open_pg(url: &str, kms_path: &Path) -> PostgresStore {
    let kms: Arc<dyn DekProvider> =
        Arc::new(LocalDevKms::open_persistent(kms_path).expect("open the file key store"));
    PostgresStore::open(url, kms).expect("open the postgres store")
}

/// Seed a SQLite deployment with the given `(item, subject, gist)` triples.
pub fn seed_sqlite(db: &Path, kms: &Path, items: &[(&str, &str, &str)]) {
    std::fs::create_dir_all(db.parent().expect("the db has a parent")).expect("create the db dir");
    let store = open_sqlite(db, kms);
    for (id, subject, gist) in items {
        write_item(&store, id, subject, gist);
    }
}

/// Seed a Postgres deployment with the given `(item, subject, gist)` triples.
pub fn seed_pg(url: &str, kms: &Path, items: &[(&str, &str, &str)]) {
    let store = open_pg(url, kms);
    for (id, subject, gist) in items {
        write_item(&store, id, subject, gist);
    }
}

/// A journal path under the scratch root whose parent directory exists, so a
/// refusal that never writes still has a place the authority could live.
pub fn journal_file(scratch: &common::Scratch, relative: &str) -> PathBuf {
    let path = scratch.join(relative);
    std::fs::create_dir_all(path.parent().expect("the journal path has a parent"))
        .expect("create the journal directory");
    path
}

/// The backup directory a scenario's archives live in.
pub fn backup_dir(scratch: &common::Scratch) -> PathBuf {
    let path = scratch.join("backups");
    std::fs::create_dir_all(&path).expect("create the backup directory");
    path
}
