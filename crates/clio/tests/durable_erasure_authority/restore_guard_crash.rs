// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for measuring the guarded restore's crash
//! recovery across a real process boundary: a spawned child records the durable
//! intent and places, is killed before it can settle, and the parent reopens
//! the durable event store and reconciles the one pending action without
//! repeating placement.
//!
//! ## Owns
//! - The child helper that places and then waits to be killed
//! - The kill-and-reap guard and the bounded placed-marker wait
//! - The cross-process crash-recovery scenario
//!
//! ## Does not own
//! - The in-process recovery scenarios (see `restore_guard_recovery`)
//! - The fixtures (see `restore_guard_fixtures`)
//!
//! ## Boundary
//!
//! Changes to how a cross-process crash is driven should normally require
//! changes here. Changes to the guard sequence should not.
//!
//! Keep this module focused on the cross-process crash and restart.

use crate::restore_guard_fixtures::{claim_of, event_for, events_paths, place, request, target};
use crate::support::{BANK, Scratch, denied_hash, journal_path, open_sqlite};
use clio::store::{LocalEventState, LocalEventStore};
use clio_compliance::{MemberFacts, begin_restore, reconcile_restore_actions, record_denial};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The env var that turns this test binary's child run into a placing crash.
const CHILD_ENV: &str = "CLIO_DURABLE_ERASURE_CRASH_CHILD";

/// The child test's name, used as the spawn filter. It is unique in this
/// target, so a substring filter selects exactly the child run.
const CHILD_TEST: &str = "child_places_then_waits_to_be_killed";

/// How long the child waits to be killed before giving up on its own, so a lost
/// parent cannot leave it running forever.
const CHILD_WAIT: Duration = Duration::from_secs(60);

/// How long the parent waits for the child's placed marker.
const MARKER_WAIT: Duration = Duration::from_secs(30);

/// The child half of the crash. Run as an ordinary test with no env var, it
/// returns immediately, so it is a harmless no-op in the normal suite. Spawned
/// with `CHILD_ENV`, it opens the durable store, begins the restore, places,
/// signals the parent, and waits to be killed before it can settle.
#[test]
fn child_places_then_waits_to_be_killed() {
    let Ok(brief) = std::env::var(CHILD_ENV) else {
        return;
    };
    let mut lines = brief.lines();
    let journal = PathBuf::from(lines.next().expect("child journal path"));
    let db = PathBuf::from(lines.next().expect("child events db path"));
    let kms = PathBuf::from(lines.next().expect("child events kms path"));
    let target_db = PathBuf::from(lines.next().expect("child target path"));
    let marker = PathBuf::from(lines.next().expect("child marker path"));
    let action_id = lines.next().expect("child action id");

    let store = open_sqlite(&db, &kms);
    let claim = claim_of(&journal);
    let guard = begin_restore(
        &store,
        &request(&journal, &claim, &[MemberFacts::None], None, action_id),
    )
    .expect("the child's intent and authority pass");
    place(&target_db);
    std::fs::write(&marker, b"placed").expect("signal the parent");
    let deadline = Instant::now() + CHILD_WAIT;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(guard);
}

/// A spawned child that is killed and reaped even when the test panics.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Wait for `path` to appear, with a deadline so a lost handshake fails instead
/// of hanging.
fn wait_for_file(path: &Path, limit: Duration) {
    let deadline = Instant::now() + limit;
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "{} never appeared",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A real crash: the child places and is killed before it can settle, leaving
/// exactly one durable pending action that a reopened store reconciles once
/// without repeating placement.
#[test]
fn a_killed_child_that_placed_leaves_one_pending_action_a_restart_reconciles_once() {
    let scratch = Scratch::new("guard-api-child-crash");
    let journal = journal_path(&scratch, "journal.json");
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record a denial");
    let (db, kms) = events_paths(&scratch, "events");
    let (target_db, _kms) = target(&scratch, "target");
    let marker = scratch.join("child-placed");
    let action_id = "restore-child-crash";
    let brief = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        journal.display(),
        db.display(),
        kms.display(),
        target_db.display(),
        marker.display(),
        action_id,
    );
    let stdout = std::fs::File::create(scratch.join("child-stdout.log")).expect("child stdout");
    let stderr = std::fs::File::create(scratch.join("child-stderr.log")).expect("child stderr");
    let mut child = ChildGuard(
        Command::new(std::env::current_exe().expect("this test binary"))
            .args([CHILD_TEST, "--nocapture"])
            .env(CHILD_ENV, &brief)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("spawn the placing child"),
    );
    wait_for_file(&marker, MARKER_WAIT);

    // Kill the child before it can settle, so the durable intent it recorded
    // survives with no outcome. Reap it before reopening the store.
    child.0.kill().expect("kill the child");
    let status = child.0.wait().expect("reap the child");
    assert!(
        !status.success(),
        "the child was killed, not a clean exit: {status}"
    );

    // Restart: reopen the durable event store the child wrote, then reconcile
    // the one pending action it left. The reopen lets SQLite recover whatever
    // the killed child left behind.
    let store = open_sqlite(&db, &kms);
    assert_eq!(
        store.pending_actions().expect("pending"),
        vec![action_id.to_owned()],
        "the killed child left exactly one durable pending action"
    );
    let reconciled = reconcile_restore_actions(&store).expect("reconcile after restart");
    assert_eq!(
        reconciled,
        vec![action_id.to_owned()],
        "the restart closes the pending action the killed child left"
    );
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "the pending action is closed after reconcile"
    );
    let events = event_for(&store, action_id);
    assert_eq!(events.len(), 1, "one terminal event after reconcile");
    assert_eq!(events[0].state, LocalEventState::Denied);
    assert_eq!(
        events[0].result.as_deref(),
        Some("reconciled after restart without repeating placement"),
        "the terminal event records the reconciliation"
    );
    println!("RECONCILED guard-api child-crash action={action_id}");
}
