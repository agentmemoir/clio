// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared fixtures and probes of the peer
//! erasure acceptance target: the synthetic bank, subject, and timestamp, the
//! per-test scratch directory, the store surface both backends implement, the
//! synthetic item writer, and the read-only probes over a coordinator or peer
//! store and a peer's erasure journal.
//!
//! ## Owns
//! - `Scratch`, `unique`, and the synthetic constants
//! - `AcceptanceStore` and `EraseBankSet`: the store surface the generic
//!   scenario bodies drive
//! - `item` and `write_item`, the synthetic subject data
//! - `PeerStore` and `journal_facts`, the peer-side observations
//!
//! ## Does not own
//! - The coordinator fixture and lifecycle calls (`run`)
//! - The peer process and its logs (`sync_wire_support::peer`)
//! - The injected event-writer faults (`faults`)
//! - What each scenario asserts (`sqlite`, `postgres`)
//!
//! ## Boundary
//!
//! Changes to how a fixture is seeded or observed should normally require
//! changes here. Changes to a scenario's assertions should not.
//!
//! Keep this module focused on fixtures and probes, not scenario logic.

// Every scenario module of this target uses a subset of these fixtures; the
// target compiles as one binary, so an unused-in-one-suite helper is not dead.
#![allow(dead_code)]

use clio::store::{
    ComplianceStore, DekProvider, DurableRequestStore, LocalDevKms, LocalEventStore,
    PeerActionStore, PostgresStore, SqliteStore, Store, SyncStore, connect_postgres,
};
use clio::{AmError, ItemKind, MemoryItem, SemanticCategory};
use clio_compliance::{JournalState, load_journal};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The bank holding the erased subject's data.
pub const BANK: &str = "acceptance-bank";
/// A second bank no denial covers: the positive control.
pub const CONTROL_BANK: &str = "acceptance-control-bank";
/// The subject the coordinated erasure and every denial name.
pub const SUBJECT: &str = "acceptance-subject";
/// A second subject in the control bank, the positive control's identity.
pub const CONTROL_SUBJECT: &str = "acceptance-control-subject";
/// The banks every peer serves and every credential reaches.
pub const BANKS: [&str; 2] = [BANK, CONTROL_BANK];
/// The fixed transaction stamp the fixtures use; never a real clock.
pub const NOW: &str = "2026-01-01T00:00:00Z";
/// The Postgres URL used when the environment names none, matching the
/// Makefile and the other loopback-Postgres harnesses.
const COMPOSE_PG: &str = "postgres://clio:clio@127.0.0.1:34310/clio";

/// The store surface one acceptance run drives: every backend implements all
/// of these, so one scenario body runs on SQLite and Postgres unchanged.
pub trait AcceptanceStore:
    Store
    + SyncStore
    + ComplianceStore
    + DurableRequestStore
    + PeerActionStore
    + LocalEventStore
    + EraseBankSet
    + Send
    + Sync
{
}

impl<T> AcceptanceStore for T where
    T: Store
        + SyncStore
        + ComplianceStore
        + DurableRequestStore
        + PeerActionStore
        + LocalEventStore
        + EraseBankSet
        + Send
        + Sync
{
}

/// The live bank set one subject's key is shared across, read from the
/// effect's own store. The method is inherent on each backend, so this test
/// trait is the one handle a generic scenario body can call.
pub trait EraseBankSet {
    /// Every bank the subject has rows in, the coordination check's input.
    ///
    /// # Errors
    /// A storage failure.
    fn live_banks(&self, bank: &str, subject: &str) -> Result<Vec<String>, AmError>;
}

impl EraseBankSet for SqliteStore {
    fn live_banks(&self, bank: &str, subject: &str) -> Result<Vec<String>, AmError> {
        self.erase_bank_set(bank, subject)
    }
}

impl EraseBankSet for PostgresStore {
    fn live_banks(&self, bank: &str, subject: &str) -> Result<Vec<String>, AmError> {
        self.erase_bank_set(bank, subject)
    }
}

/// The deployment fingerprint both the driver and its peer processes verify
/// denials under, resolved from the driver's own environment so a host that
/// overrides the deployment salt keeps both sides identical.
#[must_use]
pub fn fingerprint() -> String {
    clio_compliance::tombstone_salt_fingerprint()
}

/// A per-test temp directory, removed on drop.
pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// Create a fresh, unique temp directory for one scenario.
    #[must_use]
    pub fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(unique(tag));
        std::fs::create_dir_all(&path).expect("create the scratch directory");
        Self { path }
    }

    /// Join a relative path under the scratch directory.
    #[must_use]
    pub fn join(&self, relative: &str) -> PathBuf {
        self.path.join(relative)
    }

    /// The scratch root itself.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A unique string per call, so two tests never share a path or a name.
#[must_use]
pub fn unique(tag: &str) -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("peer-acceptance-{tag}-{}-{sequence}", std::process::id())
}

/// One synthetic item carrying `subject` in `bank`.
#[must_use]
pub fn item(id: &str, bank: &str, subject: &str) -> MemoryItem {
    MemoryItem {
        id: id.to_owned(),
        bank_id: bank.to_owned(),
        subject_id: subject.to_owned(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: clio::EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.8),
        source_ref: None,
        snapshot: Some(serde_json::json!({"entity": "synthetic acceptance subject"})),
        gist: Some("synthetic acceptance gist".into()),
        valid_from: NOW.into(),
        valid_until: None,
        tx_from: NOW.into(),
        tx_until: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        context: None,
    }
}

/// Write one synthetic item into an open node store.
pub fn write_item<T: Store>(node: &T, id: &str, bank: &str, subject: &str) {
    node.create_memory_item(&item(id, bank, subject), "harness")
        .expect("write the synthetic item");
}

/// One peer store the scenarios probe: the SQLite file or the Postgres schema
/// the peer process writes.
pub enum PeerStore {
    /// The peer's SQLite database file.
    Sqlite(PathBuf),
    /// The peer's private Postgres schema URL.
    Postgres(String),
}

/// One quarantined mutation as a peer recorded it.
pub struct PeerDeadLetter {
    /// The peer's idempotency key for the quarantined mutation.
    pub sync_mutation_id: String,
    /// The peer's own content-free reason classification.
    pub reason_code: String,
    /// The quarantine row's status.
    pub status: String,
}

impl PeerStore {
    /// Every quarantine row the peer recorded, sorted by mutation id.
    ///
    /// # Panics
    /// The backend cannot be queried.
    #[must_use]
    pub fn dead_letters(&self) -> Vec<PeerDeadLetter> {
        match self {
            Self::Sqlite(path) => {
                let conn = Connection::open(path).expect("open the peer's database");
                conn.execute_batch("PRAGMA query_only = ON")
                    .expect("make the peer read query-only");
                let mut stmt = conn
                    .prepare(
                        "select sync_mutation_id, reason_code, status from sync_dead_letter \
                         order by sync_mutation_id",
                    )
                    .expect("prepare the quarantine read");
                stmt.query_map([], |row| {
                    Ok(PeerDeadLetter {
                        sync_mutation_id: row.get(0)?,
                        reason_code: row.get(1)?,
                        status: row.get(2)?,
                    })
                })
                .expect("query the quarantine rows")
                .collect::<Result<Vec<_>, _>>()
                .expect("quarantine rows")
            }
            Self::Postgres(url) => {
                let mut conn = connect_postgres(url).expect("connect the peer's schema");
                let rows = conn
                    .query(
                        "select sync_mutation_id, reason_code, status from sync_dead_letter \
                         order by sync_mutation_id",
                        &[],
                    )
                    .expect("query the quarantine rows");
                rows.iter()
                    .map(|row| PeerDeadLetter {
                        sync_mutation_id: row.get(0),
                        reason_code: row.get(1),
                        status: row.get(2),
                    })
                    .collect()
            }
        }
    }

    /// The number of item rows the peer holds for one subject in one bank.
    ///
    /// # Panics
    /// The backend cannot be queried.
    #[must_use]
    pub fn subject_items(&self, bank: &str, subject: &str) -> i64 {
        self.count(&format!(
            "select count(*) from items where bank_id = '{bank}' and subject_id = '{subject}'"
        ))
    }

    /// The number of subject-key rows the peer holds for one subject.
    ///
    /// # Panics
    /// The backend cannot be queried.
    #[must_use]
    pub fn subject_keys(&self, subject: &str) -> i64 {
        self.count(&format!(
            "select count(*) from subject_keys where subject_id = '{subject}'"
        ))
    }

    fn count(&self, sql: &str) -> i64 {
        match self {
            Self::Sqlite(path) => {
                let conn = Connection::open(path).expect("open the peer's database");
                conn.execute_batch("PRAGMA query_only = ON")
                    .expect("make the peer read query-only");
                conn.query_row(sql, [], |row| row.get(0))
                    .expect("probe the peer's store")
            }
            Self::Postgres(url) => {
                let mut conn = connect_postgres(url).expect("connect the peer's schema");
                let row = conn.query_one(sql, &[]).expect("probe the peer's store");
                row.get(0)
            }
        }
    }
}

/// The peer's journal generation, its recorded denials, and its fingerprint,
/// or `None` when no journal exists yet.
///
/// # Panics
/// A journal exists but does not load as the current state.
#[must_use]
pub fn journal_facts(path: &Path) -> Option<(u64, usize, Option<String>)> {
    match load_journal(path) {
        JournalState::Missing => None,
        JournalState::Current(journal) => Some((
            journal.generation,
            journal.denials.len(),
            journal.salt_fingerprint,
        )),
        JournalState::Damaged => panic!("the peer's journal at {} is damaged", path.display()),
    }
}

/// The Postgres URL to drive: the environment's, or the loopback Compose
/// database.
#[must_use]
pub fn postgres_url() -> String {
    std::env::var("CLIO_DATABASE_URL").unwrap_or_else(|_| COMPOSE_PG.to_owned())
}

/// A process-local key provider for every fixture store.
#[must_use]
pub fn local_kms() -> Arc<dyn DekProvider> {
    Arc::new(LocalDevKms::new())
}
