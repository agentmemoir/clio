// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the read-only probe over the store a spawned
//! peer writes: the quarantine rows it recorded and the item rows it holds for
//! one subject.
//!
//! ## Owns
//! - `PeerProbe`, its quarantine-reason read, and its per-subject item count
//!
//! ## Does not own
//! - The peer process and the store it writes
//!   (`peer_process_test_support`)
//! - What any scenario asserts about the probe's answers
//!
//! ## Boundary
//!
//! Changes to what a probe reads back should normally require changes here.
//! Changes to how a peer is spawned should not.
//!
//! Keep this module focused on reading a peer's stored state.

use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// A read-only probe of the store a peer writes: its quarantined mutation
/// reasons and its per-subject item counts.
pub struct PeerProbe {
    db: PathBuf,
}

impl PeerProbe {
    /// A probe over the SQLite file `db`.
    #[must_use]
    pub fn new(db: &Path) -> Self {
        Self {
            db: db.to_path_buf(),
        }
    }

    /// Every quarantine reason the peer recorded, sorted by mutation id.
    ///
    /// # Panics
    /// The peer's database cannot be queried.
    #[must_use]
    pub fn dead_letter_reasons(&self) -> Vec<String> {
        let conn = Connection::open(&self.db).expect("open the peer database");
        conn.execute_batch("PRAGMA query_only = ON")
            .expect("make the peer query-only");
        let mut stmt = conn
            .prepare("select reason_code from sync_dead_letter order by sync_mutation_id")
            .expect("prepare the quarantine read");
        stmt.query_map([], |row| row.get::<_, String>(0))
            .expect("query the quarantine rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("quarantine rows")
    }

    /// The number of item rows the peer holds for one subject in one bank.
    ///
    /// # Panics
    /// The peer's database cannot be queried.
    #[must_use]
    pub fn subject_items(&self, bank: &str, subject: &str) -> i64 {
        let conn = Connection::open(&self.db).expect("open the peer database");
        conn.execute_batch("PRAGMA query_only = ON")
            .expect("make the peer query-only");
        conn.query_row(
            "select count(*) from items where bank_id = ?1 and subject_id = ?2",
            rusqlite::params![bank, subject],
            |row| row.get(0),
        )
        .expect("probe the peer's items")
    }
}
