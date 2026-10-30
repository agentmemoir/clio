// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the fixtures the guarded-restore API
//! scenarios share: the fault-injecting event-store wrapper, the durable event
//! store, the restore request builder, the placement fixture, and the event
//! read-back.
//!
//! ## Owns
//! - `Fault` and its intent/outcome failure constructors
//! - `events_store`, `claim_of`, `request`, `target`, `place`, and `event_for`
//!
//! ## Does not own
//! - The guard sequence (`clio-compliance` owns `begin_restore`)
//! - The scenarios and their assertions (see `restore_guard` and
//!   `restore_guard_recovery`)
//!
//! ## Boundary
//!
//! Changes to how a guard scenario is driven should normally require changes
//! here. Changes to a scenario's assertions should not.
//!
//! Keep this module focused on the shared guard fixtures, with no tests.

use crate::support::{DENIED_SUBJECT, Scratch, load_current, open_sqlite, write_item};
use clio::store::{
    DestinationCustodyRow, LocalEventIntent, LocalEventStore, LocalEventView, SqliteStore,
    SwapRequest, swap_sqlite_store,
};
use clio_compliance::{ArchiveClaim, CopyCustodyFacts, JournalState, MemberFacts, RestoreRequest};
use std::path::{Path, PathBuf};

/// The event time every fixture records, so the read-back is deterministic.
pub(crate) const NOW: &str = "2026-09-17T11:00:00Z";

/// The actor a guarded restore records.
pub(crate) const ACTOR: &str = "system";

/// An injected event-writer fault over a borrowed store, mirroring the
/// export/review fault seam so a failed intent and a failed outcome can be
/// driven through the real `begin_restore` and `RestoreGuard`.
pub(crate) struct Fault<'a> {
    inner: &'a dyn LocalEventStore,
    fail_intent: bool,
    fail_outcome: bool,
}

impl<'a> Fault<'a> {
    /// An intent write that always fails before any effect.
    pub(crate) fn intent_fails(inner: &'a dyn LocalEventStore) -> Self {
        Self {
            inner,
            fail_intent: true,
            fail_outcome: false,
        }
    }

    /// A real store that records intents but fails every outcome write.
    pub(crate) fn outcome_fails(inner: &'a dyn LocalEventStore) -> Self {
        Self {
            inner,
            fail_intent: false,
            fail_outcome: true,
        }
    }
}

impl LocalEventStore for Fault<'_> {
    fn record_intent(&self, intent: &LocalEventIntent<'_>) -> Result<(), clio::AmError> {
        if self.fail_intent {
            return Err(clio::AmError::new(
                clio::ErrorCode::Internal,
                "injected intent-write failure",
            ));
        }
        self.inner.record_intent(intent)
    }

    fn record_outcome(
        &self,
        action_id: &str,
        denied: bool,
        result: &str,
    ) -> Result<(), clio::AmError> {
        if self.fail_outcome {
            return Err(clio::AmError::new(
                clio::ErrorCode::Internal,
                "injected outcome-write failure",
            ));
        }
        self.inner.record_outcome(action_id, denied, result)
    }

    fn pending_actions(&self) -> Result<Vec<String>, clio::AmError> {
        self.inner.pending_actions()
    }

    fn list_events(&self) -> Result<Vec<LocalEventView>, clio::AmError> {
        self.inner.list_events()
    }

    fn prune_completed_before(&self, cutoff: &str) -> Result<usize, clio::AmError> {
        self.inner.prune_completed_before(cutoff)
    }
}

/// The durable event-store database and key-store paths for `tag`, with the
/// parent directory created, but nothing opened. A second process uses these to
/// open the same durable store this process later reopens.
pub(crate) fn events_paths(scratch: &Scratch, tag: &str) -> (PathBuf, PathBuf) {
    let db = scratch.join(&format!("{tag}/events.db"));
    std::fs::create_dir_all(db.parent().expect("the events db has a parent"))
        .expect("create the events directory");
    let kms = scratch.join(&format!("{tag}/events.kms.json"));
    (db, kms)
}

/// Open the durable event store the guarded restore would write to.
pub(crate) fn events_store(scratch: &Scratch, tag: &str) -> SqliteStore {
    let (db, kms) = events_paths(scratch, tag);
    open_sqlite(&db, &kms)
}

/// The claim the live journal would put on an archive made at its authority.
pub(crate) fn claim_of(journal: &Path) -> ArchiveClaim {
    let current = load_current(journal);
    ArchiveClaim {
        deployment_id: current.deployment_id.clone(),
        generation: current.generation,
        salt_fingerprint: Some(clio_compliance::tombstone_salt_fingerprint()),
    }
}

/// A restore request over one journal, claim, and member set.
pub(crate) fn request<'a>(
    journal: &'a Path,
    claim: &'a ArchiveClaim,
    members: &'a [MemberFacts],
    presented: Option<&'a JournalState>,
    action_id: &'a str,
) -> RestoreRequest<'a> {
    // The custody facts pass the copy-custody gate: the row's digest matches
    // the copy and its receiver is the acting restorer.
    RestoreRequest {
        journal_path: journal,
        presented,
        claim,
        members,
        copy_custody: CopyCustodyFacts {
            row: Some(DestinationCustodyRow {
                action_id: action_id.to_owned(),
                destination_path: "copy".to_owned(),
                custody_owner: "owner".to_owned(),
                required_action: "erase".to_owned(),
                state: "delivered".to_owned(),
                exception_reason: None,
                copy_digest: Some(
                    "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                        .to_owned(),
                ),
                receiver: Some(ACTOR.to_owned()),
                updated_at: NOW.to_owned(),
            }),
            copy_digest: "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            restorer: ACTOR,
        },
        actor: ACTOR,
        action_id,
        now: NOW,
    }
}

/// A target store with one permitted item, and its paths.
pub(crate) fn target(scratch: &Scratch, tag: &str) -> (PathBuf, PathBuf) {
    let db = scratch.join(&format!("{tag}/target.db"));
    let kms = scratch.join(&format!("{tag}/target.kms.json"));
    std::fs::create_dir_all(db.parent().expect("the target has a parent"))
        .expect("create the target directory");
    let store = open_sqlite(&db, &kms);
    write_item(&store, "target-live", "subject-live", "live target gist");
    drop(store);
    (db, kms)
}

/// Place a staged store over `target` with the file swap, the effect the guard
/// authorizes; the caller already holds the guard. The staged file lives beside
/// the target so the placement stays on one filesystem.
pub(crate) fn place(target: &Path) {
    let staged = target.with_file_name("staged.db");
    let staged_kms = target.with_file_name("staged.kms.json");
    let store = open_sqlite(&staged, &staged_kms);
    write_item(&store, "staged-item", DENIED_SUBJECT, "staged gist");
    drop(store);
    swap_sqlite_store(&SwapRequest {
        staged_payload: staged,
        target: target.to_path_buf(),
        key: None,
    })
    .expect("swap the staged store over the target");
}

/// Every event row for `action_id`, in order position.
pub(crate) fn event_for(store: &dyn LocalEventStore, action_id: &str) -> Vec<LocalEventView> {
    let mut rows: Vec<LocalEventView> = store
        .list_events()
        .expect("list events")
        .into_iter()
        .filter(|event| event.action_id == action_id)
        .collect();
    rows.sort_by_key(|event| event.seq);
    rows
}
