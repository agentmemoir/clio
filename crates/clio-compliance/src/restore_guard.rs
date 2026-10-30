// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the guarded restore: the failure-closed
//! authority check a restore runs at planning and again under a journal lock,
//! the durable restore intent recorded before any placement, the placement
//! guard that holds the lock, and the reconciliation of a pending restore a
//! restart leaves.
//!
//! ## Owns
//! - `check_restore_at_plan`, `begin_restore`, `RestoreGuard`, `JournalHold`
//! - `RestoreGuardRefusal`, its error code, and `reconcile_restore_actions`
//! - The restore sequence: journal lock, reconcile under it, durable intent,
//!   authority re-check under the lock, and the settle
//! - The one deployment erasure-journal path resolution the restore CLI and
//!   the erase boundary share
//!
//! ## Does not own
//! - The journal model and the authority rules (`erasure_journal` owns those)
//! - The durable event writer (`clio-store` owns `LocalEventStore`)
//! - The exclusive hold and the lock file (`clio-store` owns `ExclusiveHold`)
//! - What placement does: this module holds the lock across it and never
//!   performs it
//!
//! ## Boundary
//!
//! Changes to how a restore is sequenced or reconciled should normally require
//! changes here. Changes to the authority rules or the event writer should not.
//!
//! Keep this module focused on the guarded restore sequence.
//!
//! # Why the lock and the intent
//!
//! An erasure denial takes the same exclusive journal lock, so holding it
//! across placement stops a denial from landing between the authority check and
//! the write it authorizes. The lock is taken first and every pending restore
//! action is reconciled under it, because the lock proves no peer is mid
//! placement. The durable intent is written only after that, so a crash after
//! the intent leaves a pending action that the next restore finds under the same
//! lock and closes without repeating placement.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::bundle::sha256_hex_bytes;
use crate::erasure_journal::{
    ArchiveClaim, AuthorityRefusal, JournalState, MemberFacts, check_archive_authority,
    load_journal,
};
use crate::subject_request_events::{RestoreInputs, RestoreRefusal, check_restore};
use crate::subject_request_manifest::DIGEST_ALGORITHM_PREFIX;
use clio_store::{DestinationCustodyRow, ExclusiveHold, LocalEventIntent, LocalEventStore};
use clio_types::{AmError, ErrorCode};

/// The event kind label every guarded restore writes.
pub const RESTORE_EVENT_KIND: &str = "restore_guard";

/// The opaque event scope label every guarded restore writes.
pub const RESTORE_EVENT_SCOPE: &str = "restore_authority";

/// The outcome text recorded for a restore refused before placement.
const REFUSED_BEFORE_PLACEMENT: &str = "refused before placement";

/// The outcome text recorded when reconciliation closes a pending restore.
const RECONCILED_AFTER_RESTART: &str = "reconciled after restart without repeating placement";

/// The journal lock a restore holds across placement.
///
/// It is the same exclusive hold an erasure denial takes on the journal path,
/// so placement and denial cannot interleave. It is released on drop.
pub struct JournalHold {
    _hold: ExclusiveHold,
}

impl JournalHold {
    /// Take the journal lock derived from `journal_path`, waiting a bounded
    /// time for every holder to clear.
    ///
    /// # Errors
    /// The lock is already held by this process, the lock file cannot be
    /// opened, or another process held it for the whole bounded wait.
    pub fn acquire(journal_path: &Path) -> Result<Self, AmError> {
        Ok(Self {
            _hold: ExclusiveHold::acquire(journal_path)?,
        })
    }

    /// Resolve the one erasure-journal path this deployment uses.
    ///
    /// A host-supplied effective document that names the journal is accepted
    /// only when it equals the deployment resolution: `CLIO_ERASURE_JOURNAL_FILE`
    /// when set, else the journal beside the resolved data directory. A document
    /// that names a different path is refused, because the restore CLI cannot
    /// see that document and would take a different lock. The restore CLI and
    /// the MCP erase boundary both resolve through this one function, so the
    /// lock one takes is the lock the other consults.
    ///
    /// # Errors
    /// [`ErrorCode::ConfigCorrupt`] when a document-named path differs from the
    /// deployment resolution, or when no document path is given and no absolute
    /// data directory can be resolved. The message names the override that fixes
    /// a divergence.
    pub fn resolve_journal_path(
        document: Option<&Path>,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<PathBuf, AmError> {
        match document {
            Some(path) => {
                let deployment = clio_config::resolve_journal_path(&|key: &str| get(key))?;
                if deployment == path {
                    Ok(path.to_path_buf())
                } else {
                    Err(AmError::new(
                        ErrorCode::ConfigCorrupt,
                        format!(
                            "the erasure journal named by the effective document ({}) differs \
                             from the deployment resolution ({}); set {} to the same path",
                            path.display(),
                            deployment.display(),
                            clio_config::ERASURE_JOURNAL_FILE_ENV,
                        ),
                    ))
                }
            }
            None => clio_config::resolve_journal_path(&|key: &str| get(key)),
        }
    }
}

/// Why a guarded restore did not begin.
///
/// The authority variant carries the failure-closed check's own refusal; the
/// event and lock variants name the step that failed before any placement.
#[derive(Debug)]
pub enum RestoreGuardRefusal {
    /// The authority check refused the restore.
    Authority(AuthorityRefusal),
    /// The copy-custody check refused the restore.
    Custody(RestoreRefusal),
    /// The durable event write failed: the restore intent could not be
    /// written, or the pending-action reconcile under the lock failed.
    Event(AmError),
    /// The journal lock could not be taken, so nothing was placed.
    Lock(AmError),
}

/// The stable code and operator text for a copy-custody refusal.
///
/// One mapping for the plan-time refusal and the placement-time guard refusal,
/// so the two spellings cannot drift.
#[must_use]
pub fn custody_refusal_parts(refusal: RestoreRefusal) -> (ErrorCode, String) {
    match refusal {
        RestoreRefusal::CopyManifestMissing => (
            ErrorCode::NotFound,
            "no copy-custody record backs this restore; a restore places only a copy whose \
             custody was recorded"
                .to_owned(),
        ),
        RestoreRefusal::CopyDigestMismatch => (
            ErrorCode::Forbidden,
            "the copy's digest does not match the digest its custody record holds".to_owned(),
        ),
        RestoreRefusal::ReceiverMismatch => (
            ErrorCode::Forbidden,
            "the acting restorer is not the receiver the copy's custody record names".to_owned(),
        ),
    }
}

impl RestoreGuardRefusal {
    /// The refusal's stable code in the crate's error vocabulary.
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Authority(refusal) => refusal.code(),
            Self::Custody(refusal) => custody_refusal_parts(*refusal).0,
            Self::Event(_) | Self::Lock(_) => ErrorCode::Internal,
        }
    }
}

impl fmt::Display for RestoreGuardRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Authority(refusal) => {
                write!(f, "the restore authority check refused: {refusal}")
            }
            Self::Custody(refusal) => {
                write!(
                    f,
                    "the copy-custody check refused: {}",
                    custody_refusal_parts(*refusal).1
                )
            }
            Self::Event(cause) => write!(f, "the durable restore event write failed: {cause}"),
            Self::Lock(cause) => write!(f, "the journal lock could not be taken: {cause}"),
        }
    }
}

impl std::error::Error for RestoreGuardRefusal {}

/// Run the authority check for a restore at planning time.
///
/// The journal at `journal_path` is loaded and checked with
/// `check_archive_authority`. `presented` is the authority the archive carries.
/// A same-host restore has no externally transferred authority, so a caller
/// that passes `None` presents the freshly loaded current state itself; a
/// caller with a transferred authority passes that authority explicitly.
///
/// # Errors
/// The [`AuthorityRefusal`] naming the first refusal, in check order.
pub fn check_restore_at_plan(
    journal_path: &Path,
    presented: Option<&JournalState>,
    claim: &ArchiveClaim,
    members: &[MemberFacts],
) -> Result<(), AuthorityRefusal> {
    let state = load_journal(journal_path);
    check_archive_authority(&state, presented.or(Some(&state)), claim, members)
}

/// The placement guard a begun restore holds.
///
/// It holds the journal lock for as long as it lives, so an erasure denial
/// cannot interleave with placement. [`RestoreGuard::settle`] records the
/// outcome and releases the lock by consuming the guard.
pub struct RestoreGuard {
    _hold: JournalHold,
    action_id: String,
}

impl RestoreGuard {
    /// The durable action id the restore intent was recorded under.
    #[must_use]
    pub fn action_id(&self) -> &str {
        &self.action_id
    }

    /// Record the restore outcome and release the journal lock.
    ///
    /// `allowed` is recorded as the outcome: an allowed placement writes
    /// `allowed`, a refused one writes `denied`. The guard is consumed, so the
    /// lock is released however this call returns.
    ///
    /// # Errors
    /// [`ErrorCode::NotFound`](clio_types::ErrorCode::NotFound) when no intent
    /// row exists for the action, or a storage failure.
    pub fn settle(
        self,
        events: &dyn LocalEventStore,
        allowed: bool,
        result: &str,
    ) -> Result<(), AmError> {
        events.record_outcome(&self.action_id, !allowed, result)
    }
}

/// The copy-custody facts one restore is checked against.
///
/// The row is the destination-custody row backing the copy, owned so a caller
/// that resolves it can hand it to the placement re-check; the digest is
/// recomputed over the copy's bytes with the delivery write's expression; the
/// restorer is the acting identity the guarded restore records.
pub struct CopyCustodyFacts<'a> {
    /// The custody row backing the copy, when one exists.
    pub row: Option<DestinationCustodyRow>,
    /// The digest recomputed over the copy's bytes.
    pub copy_digest: &'a str,
    /// The acting restorer's identity.
    pub restorer: &'a str,
}

/// The copy digest of a copy's bytes, in the same expression the delivery
/// write records on the custody row.
#[must_use]
pub fn copy_digest_of(bytes: &[u8]) -> String {
    format!("{DIGEST_ALGORITHM_PREFIX}{}", sha256_hex_bytes(bytes))
}

/// Run the copy-custody restore gate over the facts.
///
/// An absent row, or a row whose digest and receiver were never written, is no
/// copy-custody manifest: the gate refuses `CopyManifestMissing` before the
/// digest and receiver checks run.
pub fn check_copy_custody(facts: &CopyCustodyFacts<'_>) -> Result<(), RestoreRefusal> {
    let recorded = facts
        .row
        .as_ref()
        .and_then(|row| row.copy_digest.as_deref());
    let inputs = RestoreInputs {
        manifest_present: recorded.is_some(),
        digest_verified: recorded == Some(facts.copy_digest),
        receiver_matches: facts
            .row
            .as_ref()
            .is_some_and(|row| row.receiver.as_deref() == Some(facts.restorer)),
    };
    check_restore(&inputs)
}

/// The inputs of one guarded restore.
///
/// A borrowed bundle, so a caller names each input once and `begin_restore`
/// keeps a short argument list.
pub struct RestoreRequest<'a> {
    /// The owner-only journal path the placement lock is taken on.
    pub journal_path: &'a Path,
    /// The authority the archive carries; `None` for a same-host restore.
    pub presented: Option<&'a JournalState>,
    /// What the archive records about the authority it was made under.
    pub claim: &'a ArchiveClaim,
    /// What the archive members show about subject references.
    pub members: &'a [MemberFacts],
    /// The copy-custody facts the placement re-checks under the lock.
    pub copy_custody: CopyCustodyFacts<'a>,
    /// The actor the durable intent records.
    pub actor: &'a str,
    /// The durable action id the intent is recorded under.
    pub action_id: &'a str,
    /// The event time recorded on the intent.
    pub now: &'a str,
}

/// Begin a guarded restore: take the journal lock, reconcile any pending
/// restore action under it, record the durable intent, and re-check the
/// authority under the same hold.
///
/// Order. The journal lock is taken first; a failed lock refuses with
/// [`RestoreGuardRefusal::Lock`] and writes nothing. Every pending restore
/// action is reconciled under the lock, because holding it proves no peer is
/// mid placement; a failed reconcile refuses with
/// [`RestoreGuardRefusal::Event`]. The durable restore intent is written next; a
/// failed intent refuses with [`RestoreGuardRefusal::Event`] and takes nothing
/// further. The journal is re-loaded and the authority re-checked under the
/// lock; a refusal is recorded as a denied outcome and returned as
/// [`RestoreGuardRefusal::Authority`], and the lock is dropped. On success the
/// returned guard holds the lock across placement. A same-host restore has no
/// externally transferred authority, so `presented` is `None` and the
/// re-loaded current state is presented to the check instead.
///
/// # Errors
/// The [`RestoreGuardRefusal`] naming the failed step.
pub fn begin_restore(
    events: &dyn LocalEventStore,
    request: &RestoreRequest<'_>,
) -> Result<RestoreGuard, RestoreGuardRefusal> {
    let hold = JournalHold::acquire(request.journal_path).map_err(RestoreGuardRefusal::Lock)?;
    reconcile_restore_actions(events).map_err(RestoreGuardRefusal::Event)?;
    let intent = LocalEventIntent {
        action_id: request.action_id,
        actor: request.actor,
        scope: RESTORE_EVENT_SCOPE,
        kind: RESTORE_EVENT_KIND,
        event_time: request.now,
    };
    events
        .record_intent(&intent)
        .map_err(RestoreGuardRefusal::Event)?;
    let state = load_journal(request.journal_path);
    if let Err(refusal) = check_archive_authority(
        &state,
        request.presented.or(Some(&state)),
        request.claim,
        request.members,
    ) {
        // The intent is durable; close it before dropping the lock so a
        // restart finds the refusal settled. If this write fails, the pending
        // action is left for reconciliation.
        let _ = events.record_outcome(request.action_id, true, REFUSED_BEFORE_PLACEMENT);
        return Err(RestoreGuardRefusal::Authority(refusal));
    }
    // The copy-custody gate runs beside the authority check, still under the
    // same hold: its inputs were built before the lock, so the check here is
    // pure and the lock's scope is unchanged.
    if let Err(refusal) = check_copy_custody(&request.copy_custody) {
        let _ = events.record_outcome(request.action_id, true, REFUSED_BEFORE_PLACEMENT);
        return Err(RestoreGuardRefusal::Custody(refusal));
    }
    Ok(RestoreGuard {
        _hold: hold,
        action_id: request.action_id.to_owned(),
    })
}

/// Close every pending restore action a restart left, by action id.
///
/// A pending action whose event kind is [`RESTORE_EVENT_KIND`] is a restore
/// whose outcome write failed after placement. It is recorded denied with a
/// reconciliation reason; nothing is placed, so the placement a restart already
/// performed is never repeated. A pending action of any other kind is left for
/// its owner. Idempotent: a re-run finds no pending restore action.
///
/// The caller holds the journal lock, so a pending restore action cannot belong
/// to a live peer: every restore records its intent and settles under that same
/// lock. [`begin_restore`] calls this while holding it.
///
/// # Errors
/// A pending or event read failure, or an outcome write failure.
pub fn reconcile_restore_actions(events: &dyn LocalEventStore) -> Result<Vec<String>, AmError> {
    let pending = events.pending_actions()?;
    if pending.is_empty() {
        return Ok(Vec::new());
    }
    let restore_actions: BTreeSet<String> = events
        .list_events()?
        .into_iter()
        .filter(|event| event.kind == RESTORE_EVENT_KIND)
        .map(|event| event.action_id)
        .collect();
    let mut reconciled = Vec::new();
    for action_id in pending {
        if !restore_actions.contains(&action_id) {
            continue;
        }
        events.record_outcome(&action_id, true, RECONCILED_AFTER_RESTART)?;
        reconciled.push(action_id);
    }
    Ok(reconciled)
}

#[cfg(test)]
#[path = "restore_guard_tests.rs"]
mod restore_guard_tests;

/// The copy-custody gate facts and the placement-time refusal mapping.
#[cfg(test)]
#[path = "restore_guard_custody_tests.rs"]
mod restore_guard_custody_tests;

/// The journal resolution the guard and the erase boundary share, and the
/// reconcile `begin_restore` runs under the journal lock.
#[cfg(test)]
#[path = "restore_guard_journal_tests.rs"]
mod restore_guard_journal_tests;

/// The placement-time copy-custody refusal `begin_restore` records under the
/// journal lock.
#[cfg(test)]
#[path = "restore_guard_placement_tests.rs"]
mod restore_guard_placement_tests;
