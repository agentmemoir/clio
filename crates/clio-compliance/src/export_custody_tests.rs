// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests controlled export delivery: the binding refusal, the
//! durable-intent refusal with no staging, the overwrite refusal that leaves
//! the caller file byte-identical, the non-regular-file refusals, the
//! staging-failure settlement, and a successful atomic delivery that removes
//! the staging file and marks the row delivered.
//!
//! ## Owns
//! - The in-memory store fixture, the scratch staging/destination fixture, and
//!   the injected intent-failure event store, shared with
//!   `export_custody_reconcile_tests`
//!
//! ## Does not own
//! - The delivery module under test
//! - The reconcile assertions (`export_custody_reconcile_tests` owns those)
//! - The custody and event persistence (`clio-store` owns those)
//!
//! ## Boundary
//!
//! Tests here break when the delivery sequence or the refusal set changes.
//!
//! Test-only. Never compiled into production builds.

use super::{DeliveryRefusal, DeliveryResult, DeliverySpec, deliver_export};
use crate::subject_request_events::BindingControls;
use clio_store::{
    DekProvider, DestinationCustodyStore, LocalDevKms, LocalEventIntent, LocalEventStore,
    LocalEventView, SqliteStore,
};
use clio_types::{Actor, AmError, Bank, ErrorCode, OpContext};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The time the tests pass to every delivery.
pub(super) const NOW: &str = "2026-01-01T00:00:00Z";

/// In-memory SQLite store (the `local_review_export_tests` fixture shape).
pub(super) fn open_sqlite() -> SqliteStore {
    SqliteStore::open_in_memory(Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>)
        .expect("in-memory sqlite opens")
}

/// An operation context that holds the copy controls.
fn ctx() -> OpContext {
    OpContext::new(Bank::new("export-bank"), Actor::Harness)
}

/// The controls that accept the copy for a gated delivery.
fn accepted() -> BindingControls {
    BindingControls {
        copy_controls_accepted: true,
        ..BindingControls::default()
    }
}

/// A scratch root holding the staging directory and the destination path,
/// removed on drop.
pub(super) struct Scratch {
    pub(super) root: PathBuf,
}

impl Scratch {
    pub(super) fn new() -> Self {
        let root =
            std::env::temp_dir().join(crate::test_unique::unique("clio-compliance-export-custody"));
        std::fs::create_dir_all(&root).expect("create the scratch root");
        Self { root }
    }

    fn staging_dir(&self) -> PathBuf {
        self.root.join("staging")
    }

    pub(super) fn destination(&self) -> PathBuf {
        self.root.join("out.bin")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A delivery spec with fixed owner and action.
fn spec<'a>(action_id: &'a str, destination: &'a Path, staging_dir: &'a Path) -> DeliverySpec<'a> {
    DeliverySpec {
        action_id,
        destination,
        staging_dir,
        custody_owner: "clio",
        required_action: "delivery",
        now: NOW,
    }
}

/// The entries of a staging directory, sorted; empty when it is absent.
fn staging_entries(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(read) => read.map(|entry| entry.expect("dir entry").path()).collect(),
        Err(_) => Vec::new(),
    };
    entries.sort();
    entries
}

/// A store whose intent write always fails: the injected intent fault (the
/// `local_action_probe_tests` fixture shape).
struct IntentWriteFails;

impl LocalEventStore for IntentWriteFails {
    fn record_intent(&self, _intent: &LocalEventIntent<'_>) -> Result<(), AmError> {
        Err(AmError::new(
            ErrorCode::Internal,
            "injected intent-write failure",
        ))
    }

    fn record_outcome(
        &self,
        _action_id: &str,
        _denied: bool,
        _result: &str,
    ) -> Result<(), AmError> {
        Err(AmError::new(ErrorCode::Internal, "no intent can exist"))
    }

    fn pending_actions(&self) -> Result<Vec<String>, AmError> {
        Ok(Vec::new())
    }

    fn list_events(&self) -> Result<Vec<LocalEventView>, AmError> {
        Ok(Vec::new())
    }

    fn prune_completed_before(&self, _cutoff: &str) -> Result<usize, AmError> {
        Ok(0)
    }
}

/// Without the copy controls the delivery is refused before any write.
#[test]
fn a_missing_binding_control_refuses_before_any_write() {
    let store = open_sqlite();
    let scratch = Scratch::new();
    let destination = scratch.destination();
    let staging = scratch.staging_dir();
    let result = deliver_export(
        &ctx(),
        &store,
        &store,
        &BindingControls::default(),
        &spec("act-1", &destination, &staging),
        b"payload",
    );
    assert_eq!(
        result,
        DeliveryResult::Refused(DeliveryRefusal::BindingNotAccepted)
    );
    assert!(!destination.exists(), "a refused delivery writes no file");
    assert!(store.load("act-1").expect("load").is_none());
}

/// A durable-intent failure refuses before any staging or destination write.
#[test]
fn an_intent_write_failure_refuses_before_staging() {
    let custody = open_sqlite();
    let scratch = Scratch::new();
    let destination = scratch.destination();
    let staging = scratch.staging_dir();
    let result = deliver_export(
        &ctx(),
        &IntentWriteFails,
        &custody,
        &accepted(),
        &spec("act-1", &destination, &staging),
        b"payload",
    );
    assert_eq!(
        result,
        DeliveryResult::Refused(DeliveryRefusal::DurableIntentFailed)
    );
    assert!(!destination.exists(), "no destination write");
    assert!(!staging.exists(), "no staging directory was created");
    assert!(custody.load("act-1").expect("load").is_none());
}

/// An existing caller file is refused, left byte-identical, and the staging
/// file is cleaned; the custody row is failed.
#[test]
fn an_existing_destination_is_refused_and_left_unchanged() {
    let store = open_sqlite();
    let scratch = Scratch::new();
    let destination = scratch.destination();
    let staging = scratch.staging_dir();
    std::fs::write(&destination, b"caller-bytes").expect("write the caller file");
    let result = deliver_export(
        &ctx(),
        &store,
        &store,
        &accepted(),
        &spec("act-1", &destination, &staging),
        b"payload",
    );
    assert_eq!(
        result,
        DeliveryResult::Refused(DeliveryRefusal::DestinationExists)
    );
    assert_eq!(
        std::fs::read(&destination).expect("read the caller file"),
        b"caller-bytes",
        "the caller file is never overwritten"
    );
    assert!(
        staging_entries(&staging).is_empty(),
        "the managed staging file is removed"
    );
    let row = store.load("act-1").expect("load").expect("row exists");
    assert_eq!(row.state, "failed");
    assert_eq!(
        row.exception_reason.as_deref(),
        Some("destination already exists")
    );
}

/// A second controlled delivery to a destination the first already wrote
/// refuses atomically and leaves the first file's bytes unchanged. The refusal
/// is the hard link's own `AlreadyExists`, not only the pre-check.
#[test]
fn a_second_delivery_to_the_same_destination_refuses_and_keeps_the_first_bytes() {
    let store = open_sqlite();
    let scratch = Scratch::new();
    let destination = scratch.destination();
    let staging = scratch.staging_dir();
    let first = deliver_export(
        &ctx(),
        &store,
        &store,
        &accepted(),
        &spec("act-1", &destination, &staging),
        b"first-payload",
    );
    assert_eq!(first, DeliveryResult::Delivered);
    let second = deliver_export(
        &ctx(),
        &store,
        &store,
        &accepted(),
        &spec("act-2", &destination, &staging),
        b"second-payload",
    );
    assert_eq!(
        second,
        DeliveryResult::Refused(DeliveryRefusal::DestinationExists)
    );
    assert_eq!(
        std::fs::read(&destination).expect("read the destination"),
        b"first-payload",
        "the first delivery's bytes are never overwritten"
    );
    assert!(
        staging_entries(&staging).is_empty(),
        "no staging file survives the refused second delivery"
    );
}

/// A successful delivery writes the payload once, removes the staging file,
/// and marks the row delivered with no pending event.
#[test]
fn a_successful_delivery_writes_once_and_removes_staging() {
    let store = open_sqlite();
    let scratch = Scratch::new();
    let destination = scratch.destination();
    let staging = scratch.staging_dir();
    let result = deliver_export(
        &ctx(),
        &store,
        &store,
        &accepted(),
        &spec("act-1", &destination, &staging),
        b"payload",
    );
    assert_eq!(result, DeliveryResult::Delivered);
    assert_eq!(
        std::fs::read(&destination).expect("read the destination"),
        b"payload"
    );
    assert!(
        staging_entries(&staging).is_empty(),
        "the managed staging file is removed"
    );
    let row = store.load("act-1").expect("load").expect("row exists");
    assert_eq!(row.state, "delivered");
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "the event outcome was recorded"
    );
}

/// A staging failure settles the custody row as failed and records the event
/// outcome; the destination is never written.
#[test]
fn a_staging_failure_settles_the_row_and_the_event() {
    let store = open_sqlite();
    let scratch = Scratch::new();
    let block = scratch.root.join("staging-block");
    std::fs::write(&block, b"not-a-dir").expect("write the blocking file");
    let staging = block.join("staging");
    let destination = scratch.destination();
    let result = deliver_export(
        &ctx(),
        &store,
        &store,
        &accepted(),
        &spec("act-1", &destination, &staging),
        b"payload",
    );
    assert!(
        matches!(&result, DeliveryResult::Failed(reason) if reason.contains("staging failed")),
        "the staging failure is reported: {result:?}"
    );
    assert!(!destination.exists(), "no destination write");
    let row = store.load("act-1").expect("load").expect("row exists");
    assert_eq!(row.state, "failed");
    assert!(
        row.exception_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("staging failed")),
        "the row records the staging failure: {row:?}"
    );
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "the event outcome was recorded"
    );
}

/// A directory at the destination is refused as a non-regular file and left
/// in place.
#[test]
fn a_directory_destination_is_refused_and_left_in_place() {
    let store = open_sqlite();
    let scratch = Scratch::new();
    let destination = scratch.destination();
    std::fs::create_dir_all(&destination).expect("create the directory destination");
    let staging = scratch.staging_dir();
    let result = deliver_export(
        &ctx(),
        &store,
        &store,
        &accepted(),
        &spec("act-1", &destination, &staging),
        b"payload",
    );
    assert_eq!(
        result,
        DeliveryResult::Refused(DeliveryRefusal::DestinationNotRegularFile)
    );
    assert!(destination.is_dir(), "the directory is left in place");
    assert!(
        staging_entries(&staging).is_empty(),
        "the managed staging file is removed"
    );
    let row = store.load("act-1").expect("load").expect("row exists");
    assert_eq!(row.state, "failed");
    assert_eq!(
        row.exception_reason.as_deref(),
        Some("destination is not a regular file")
    );
}

/// A symlink at the destination is refused as a non-regular file and left
/// untouched, with its target unchanged.
#[cfg(unix)]
#[test]
fn a_symlink_destination_is_refused_and_left_untouched() {
    use std::os::unix::fs::symlink;
    let store = open_sqlite();
    let scratch = Scratch::new();
    let target = scratch.root.join("target.bin");
    std::fs::write(&target, b"target-bytes").expect("write the symlink target");
    let destination = scratch.destination();
    symlink(&target, &destination).expect("create the destination symlink");
    let staging = scratch.staging_dir();
    let result = deliver_export(
        &ctx(),
        &store,
        &store,
        &accepted(),
        &spec("act-1", &destination, &staging),
        b"payload",
    );
    assert_eq!(
        result,
        DeliveryResult::Refused(DeliveryRefusal::DestinationNotRegularFile)
    );
    assert!(
        std::fs::symlink_metadata(&destination)
            .expect("inspect the symlink")
            .file_type()
            .is_symlink(),
        "the symlink is left untouched"
    );
    assert_eq!(
        std::fs::read(&target).expect("read the target"),
        b"target-bytes",
        "the symlink target is never overwritten"
    );
    let row = store.load("act-1").expect("load").expect("row exists");
    assert_eq!(row.state, "failed");
    assert_eq!(
        row.exception_reason.as_deref(),
        Some("destination is not a regular file")
    );
}
