// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for controlled export delivery: recording the
//! durable event intent and the pending destination custody row before any
//! file effect, staging bytes, delivering the file atomically without
//! overwriting a caller file, and reconciling open destinations after a
//! restart.
//!
//! ## Owns
//! - `deliver_export`, `reconcile_open_destinations`, and their result types
//! - The delivery order: durable intent, pending custody row, staging, atomic
//!   link, and the refusal, failure, and reconciliation outcomes
//!
//! ## Does not own
//! - The managed file mechanics (`export_custody_files`): staging, the
//!   destination temp, and the leftover sweep
//! - The custody-row and event persistence SQL (`clio-store` owns both)
//! - The binding gate's scope rules (`subject_request_events::check_binding`)
//! - The export selection and rendering (`export`, `local_review_export`)
//!
//! ## Boundary
//!
//! Changes to how one delivery is sequenced or reconciled should normally
//! require changes here. Changes to how managed files are named, written, or
//! swept should not.
//!
//! Keep this module focused on the controlled export delivery sequence.
//!
//! # Failure rationale
//!
//! The pending custody row is written before staging, so a crash leaves a
//! durable open row. When a failure happens before the destination link, the
//! destination was never written, so the row is marked failed. When a failure
//! happens after the link — the destination may exist — the row is left
//! pending so reconciliation finds it by action id and never overwrites it.

#[path = "export_custody_files.rs"]
pub(crate) mod export_custody_files;

use self::export_custody_files::{
    destination_present, fsync_dir, io_error, remove_staged, stage_staging, sweep_leftovers,
    write_destination_temp,
};
use crate::bundle::sha256_hex_bytes;
use crate::subject_request_events::BindingControls;
use crate::subject_request_manifest::{CopyCustodyManifest, DIGEST_ALGORITHM_PREFIX};
use clio_store::{
    DestinationCustodyRow, DestinationCustodyStore, LocalEventIntent, LocalEventStore,
};
use clio_types::{AmError, ErrorCode, OpContext};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// The event kind label every export delivery writes.
const EVENT_KIND: &str = "export_delivery";

/// The opaque event scope label every export delivery writes.
const EVENT_SCOPE: &str = "export_custody";

/// The stored delivery state of an export destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustodyState {
    /// The destination row is written and the delivery is not settled.
    Pending,
    /// The delivery completed and the destination file exists.
    Delivered,
    /// The delivery failed before the destination was written.
    Failed,
}

impl CustodyState {
    /// The stored state spelling.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Delivered => "delivered",
            Self::Failed => "failed",
        }
    }
}

/// Why a controlled delivery was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryRefusal {
    /// The binding does not hold the copy controls.
    BindingNotAccepted,
    /// The destination already exists; a caller file is never overwritten.
    DestinationExists,
    /// The destination is a symlink, a directory, or another non-regular file.
    DestinationNotRegularFile,
    /// The durable event intent could not be written, so nothing was staged.
    DurableIntentFailed,
}

/// The outcome of one controlled delivery attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryResult {
    /// The destination was written and the row marked delivered.
    Delivered,
    /// The delivery was refused before the destination was written.
    Refused(DeliveryRefusal),
    /// The delivery failed; the reason names the step.
    Failed(String),
}

/// The parameters of one controlled delivery.
pub struct DeliverySpec<'a> {
    /// The export action id; the custody row and event key on it.
    pub action_id: &'a str,
    /// The destination file to deliver to.
    pub destination: &'a std::path::Path,
    /// The product-controlled directory the bytes are staged in.
    pub staging_dir: &'a std::path::Path,
    /// The owner of the destination copy.
    pub custody_owner: &'a str,
    /// The action the destination requires before it is settled.
    pub required_action: &'a str,
    /// The time to record, RFC3339 UTC, injected so tests control the clock.
    pub now: &'a str,
}

/// One open destination found by reconciliation.
pub struct OpenDestination {
    /// The export action id.
    pub action_id: String,
    /// The recorded destination path.
    pub destination_path: String,
    /// Whether the destination exists; true means a known open copy.
    pub destination_present: bool,
    /// The last update time of the pending row.
    pub updated_at: String,
}

/// A failure part-way through the destination write.
pub(crate) enum DeliveryFailure {
    /// The destination was refused before any write.
    Refused(DeliveryRefusal),
    /// The write failed before the link; the destination was not written.
    BeforeWrite(AmError),
    /// The link happened but the directory sync failed; the destination may
    /// have been written.
    AfterWrite(AmError),
}

/// Deliver `bytes` to a new destination under durable custody control.
///
/// The event intent is recorded first; its failure refuses before any stage or
/// destination write. The pending custody row is persisted before staging; a
/// supplied copy-custody manifest records its copy digest and receiver on that
/// row, and a delivery without a manifest records the export digest over the
/// delivered bytes and the acting receiver the event intent records. The bytes
/// are staged in `staging_dir`, then delivered through a same-directory
/// temp sibling created with create-new, synced, and hard-linked onto the
/// destination, refusing an existing caller file. On success the row is marked
/// delivered and the event outcome recorded. On a failure before the link the
/// managed staging file is removed, the row is marked failed, and the event
/// outcome is recorded; the row stays pending only when the destination may
/// have been written.
#[must_use]
pub fn deliver_export(
    ctx: &OpContext,
    events: &dyn LocalEventStore,
    custody: &dyn DestinationCustodyStore,
    controls: &BindingControls,
    manifest: Option<&CopyCustodyManifest>,
    spec: &DeliverySpec<'_>,
    bytes: &[u8],
) -> DeliveryResult {
    if !controls.copy_controls_accepted {
        return DeliveryResult::Refused(DeliveryRefusal::BindingNotAccepted);
    }
    let intent = LocalEventIntent {
        action_id: spec.action_id,
        actor: ctx.actor.as_str(),
        scope: EVENT_SCOPE,
        kind: EVENT_KIND,
        event_time: spec.now,
    };
    if events.record_intent(&intent).is_err() {
        return DeliveryResult::Refused(DeliveryRefusal::DurableIntentFailed);
    }
    let (copy_digest, receiver) = match manifest {
        Some(manifest) => (
            Some(manifest.copy_digest.clone()),
            Some(manifest.receiver.clone()),
        ),
        None => (
            Some(format!(
                "{DIGEST_ALGORITHM_PREFIX}{}",
                sha256_hex_bytes(bytes)
            )),
            Some(ctx.actor.as_str().to_owned()),
        ),
    };
    let row = DestinationCustodyRow {
        action_id: spec.action_id.to_owned(),
        destination_path: spec.destination.display().to_string(),
        custody_owner: spec.custody_owner.to_owned(),
        required_action: spec.required_action.to_owned(),
        state: CustodyState::Pending.as_str().to_owned(),
        exception_reason: None,
        copy_digest,
        receiver,
        updated_at: spec.now.to_owned(),
    };
    if let Err(cause) = custody.save_pending(&row) {
        return DeliveryResult::Failed(format!("pending custody save failed: {cause}"));
    }
    let staging = match stage_staging(spec.staging_dir, spec.action_id, bytes) {
        Ok(path) => path,
        Err(cause) => {
            // The destination was never written: settle the pending row and
            // the pending event so a restart never reports an open copy.
            let reason = format!("staging failed: {cause}");
            let _ = custody.mark_failed(spec.action_id, &reason, spec.now);
            let _ = events.record_outcome(spec.action_id, true, &reason);
            return DeliveryResult::Failed(reason);
        }
    };
    match attempt_delivery(spec.destination, spec.action_id, bytes) {
        Ok(()) => {
            let recorded = custody
                .mark_delivered(spec.action_id, spec.now)
                .and_then(|()| events.record_outcome(spec.action_id, false, "export delivered"));
            remove_staged(&staging);
            match recorded {
                Ok(()) => DeliveryResult::Delivered,
                Err(cause) => DeliveryResult::Failed(format!("delivered but not settled: {cause}")),
            }
        }
        Err(DeliveryFailure::Refused(refusal)) => {
            remove_staged(&staging);
            let reason = refusal_reason(refusal);
            let _ = custody.mark_failed(spec.action_id, reason, spec.now);
            let _ = events.record_outcome(spec.action_id, true, reason);
            DeliveryResult::Refused(refusal)
        }
        Err(DeliveryFailure::BeforeWrite(cause)) => {
            remove_staged(&staging);
            let reason = cause.to_string();
            let _ = custody.mark_failed(spec.action_id, &reason, spec.now);
            let _ = events.record_outcome(spec.action_id, true, &reason);
            DeliveryResult::Failed(format!("destination write failed: {reason}"))
        }
        Err(DeliveryFailure::AfterWrite(cause)) => {
            // The destination may already exist: keep the row pending so
            // reconciliation reports it as an open copy and never redelivers.
            remove_staged(&staging);
            DeliveryResult::Failed(format!("destination may have been written: {cause}"))
        }
    }
}

/// Reconcile the export actions a restart left pending, by action id.
///
/// Every pending custody row is read back and reported as an open copy, and
/// every pending event action is matched to its destination custody row by
/// action id. The recorded destination is inspected: a file that exists means
/// the delivery landed, so the action is recorded allowed; an absent file means
/// it never landed, so the action is recorded denied. Each action's outcome is
/// recorded exactly once, an existing destination is never overwritten or
/// redelivered, and a pending action with no custody row is left for its owner.
/// The managed leftovers a crash left — the action's staged bytes and its
/// destination temp — are removed by exact managed name; no custody row is
/// changed and no destination file is touched.
///
/// # Errors
/// A custody read failure, or an event outcome write failure.
pub fn reconcile_open_destinations(
    events: &dyn LocalEventStore,
    custody: &dyn DestinationCustodyStore,
) -> Result<Vec<OpenDestination>, AmError> {
    let rows = custody.open_rows()?;
    let pending = events.pending_actions()?;
    let mut open = Vec::with_capacity(rows.len());
    let mut reconciled = BTreeSet::new();
    for row in rows {
        let destination = Path::new(&row.destination_path);
        sweep_leftovers(&row.action_id, destination);
        let present = destination_present(destination);
        events.record_outcome(&row.action_id, !present, "destination reconciled")?;
        reconciled.insert(row.action_id.clone());
        open.push(OpenDestination {
            action_id: row.action_id,
            destination_path: row.destination_path,
            destination_present: present,
            updated_at: row.updated_at,
        });
    }
    // A delivery that landed but whose outcome write failed leaves a pending
    // action and a delivered row; complete it here by its action id.
    for action_id in pending {
        if reconciled.contains(&action_id) {
            continue;
        }
        let Some(row) = custody.load(&action_id)? else {
            continue;
        };
        let destination = Path::new(&row.destination_path);
        sweep_leftovers(&action_id, destination);
        let present = destination_present(destination);
        events.record_outcome(&action_id, !present, "destination reconciled")?;
    }
    Ok(open)
}

/// The human label stored as the custody failure reason for a refusal.
fn refusal_reason(refusal: DeliveryRefusal) -> &'static str {
    match refusal {
        DeliveryRefusal::BindingNotAccepted => "copy controls not accepted",
        DeliveryRefusal::DestinationExists => "destination already exists",
        DeliveryRefusal::DestinationNotRegularFile => "destination is not a regular file",
        DeliveryRefusal::DurableIntentFailed => "durable intent write failed",
    }
}

/// Write `bytes` to `destination` without overwriting an existing file.
///
/// The pre-check classifies a symlink or a directory as a non-regular target
/// with a clear message. Existence itself is decided by the hard link: a link
/// cannot replace an existing name, so a file that appears between the
/// pre-check and the link makes the link fail with `AlreadyExists` and the
/// delivery is refused, never silently overwritten.
fn attempt_delivery(
    destination: &Path,
    action_id: &str,
    bytes: &[u8],
) -> Result<(), DeliveryFailure> {
    match fs::symlink_metadata(destination) {
        Ok(meta) if !meta.file_type().is_file() => {
            return Err(DeliveryFailure::Refused(
                DeliveryRefusal::DestinationNotRegularFile,
            ));
        }
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(DeliveryFailure::BeforeWrite(io_error(
                "inspect",
                destination,
                &err,
            )));
        }
    }
    let parent = destination.parent().filter(|p| !p.as_os_str().is_empty());
    let Some(parent) = parent else {
        return Err(DeliveryFailure::BeforeWrite(AmError::new(
            ErrorCode::InvalidArgument,
            "destination has no parent directory",
        )));
    };
    let tmp = write_destination_temp(destination, action_id, bytes)?;
    if let Err(err) = fs::hard_link(&tmp, destination) {
        let _ = fs::remove_file(&tmp);
        if err.kind() == std::io::ErrorKind::AlreadyExists {
            return Err(DeliveryFailure::Refused(DeliveryRefusal::DestinationExists));
        }
        return Err(DeliveryFailure::BeforeWrite(io_error(
            "link",
            destination,
            &err,
        )));
    }
    // The destination now names the delivered bytes; the temp name is a second
    // link to the same file, so removing it is best-effort cleanup. If a crash
    // lands before the removal, the temp sits at a managed name that
    // reconciliation sweeps.
    let _ = fs::remove_file(&tmp);
    if let Err(err) = fsync_dir(parent) {
        return Err(DeliveryFailure::AfterWrite(err));
    }
    Ok(())
}

#[cfg(test)]
#[path = "export_custody_tests.rs"]
mod export_custody_tests;

#[cfg(test)]
#[path = "export_custody_manifest_tests.rs"]
mod export_custody_manifest_tests;

#[cfg(test)]
#[path = "export_custody_reconcile_tests.rs"]
mod export_custody_reconcile_tests;

#[cfg(test)]
#[path = "export_subject_scenarios_reconcile_tests.rs"]
mod export_subject_scenarios_reconcile_tests;
