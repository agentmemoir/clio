// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one acceptance node: a spawned
//! `clio sync serve` peer process paired with the probe over the store and
//! erasure journal it writes, on either backend.
//!
//! ## Owns
//! - `Node`, the peer process plus its store probe, journal path, and logs
//! - The SQLite and Postgres node constructors
//!
//! ## Does not own
//! - The spawned process itself and its stop/resume controls
//!   (`sync_wire_support::peer`)
//! - The store and journal probes (`support`)
//! - The scenarios that drive it (`scenarios_peer`, `scenarios_disposition`)
//!
//! ## Boundary
//!
//! Changes to how one node is spawned or probed should normally require
//! changes here. Changes to what a scenario asserts about it should not.
//!
//! Keep this module focused on one probed peer node.

#![allow(dead_code)]

use clio_store::ScopedSchema;
use std::path::PathBuf;

use super::support::{PeerStore, journal_facts};
use crate::sync_wire_support::NodeDir;
use crate::sync_wire_support::peer::ServedPeer;

/// One acceptance node: a spawned peer process beside the store and journal
/// it writes, kept alive together and torn down in order.
pub struct Node {
    /// The spawned peer process, killed and reaped when this node drops.
    peer: ServedPeer,
    /// Keeps the node's scratch directory alive until after the child exits.
    _dir: NodeDir,
    /// The store probe for the backend the peer writes.
    pub store: PeerStore,
    /// The peer's erasure journal, the durable denial authority.
    pub journal: PathBuf,
}

impl Node {
    /// Start one SQLite-backed peer over a fresh database inside its own
    /// scratch directory, serving `banks`.
    #[must_use]
    pub fn sqlite(tag: &str, banks: &[&str]) -> Self {
        let dir = NodeDir::new(tag);
        let db = dir.db("peer.sqlite");
        let journal = dir.db("erasure-journal.json");
        let peer = ServedPeer::start_sqlite(&dir, &db, &journal, banks);
        Self {
            peer,
            _dir: dir,
            store: PeerStore::Sqlite(db),
            journal,
        }
    }

    /// Start one Postgres-backed peer on a private schema of `base`, serving
    /// `banks`. The schema lives exactly as long as the peer process.
    #[must_use]
    pub fn postgres(tag: &str, base: &str, banks: &[&str]) -> Self {
        let dir = NodeDir::new(tag);
        let journal = dir.db("erasure-journal.json");
        let schema = ScopedSchema::new(base);
        let url = schema.url().to_owned();
        let mut peer = ServedPeer::start_postgres(&dir, &url, &journal, banks);
        peer.hold(schema);
        Self {
            peer,
            _dir: dir,
            store: PeerStore::Postgres(url),
            journal,
        }
    }

    /// The endpoint a client connects to: a bare `host:port`.
    #[must_use]
    pub fn remote(&self) -> &str {
        self.peer.remote()
    }

    /// Stop the peer process so its endpoint answers nothing: the offline
    /// window, with the process identity kept so the resumed peer is the same
    /// deployment.
    ///
    /// # Errors
    /// The stop command cannot be run or reports a failure.
    #[cfg(unix)]
    pub fn stop(&self) -> std::io::Result<()> {
        self.peer.stop()
    }

    /// Resume a stopped peer process, restoring the same address and identity.
    ///
    /// # Errors
    /// The resume command cannot be run or reports a failure.
    #[cfg(unix)]
    pub fn resume(&self) -> std::io::Result<()> {
        self.peer.resume()
    }

    /// The peer deployment's device identity, the value its denial proofs
    /// carry and the driver's delivery target must bind.
    #[must_use]
    pub fn device_id(&self) -> &str {
        self.peer.device_id()
    }

    /// The peer's journal generation, denial count, and fingerprint.
    #[must_use]
    pub fn journal_facts(&self) -> Option<(u64, usize, Option<String>)> {
        journal_facts(&self.journal)
    }

    /// Both captured process logs, stdout first.
    #[must_use]
    pub fn logs(&self) -> String {
        self.peer.logs()
    }
}
