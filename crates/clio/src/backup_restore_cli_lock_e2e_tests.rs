// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the live-CLI transcript of the restore's
//! cross-process store lock: a store held open by a real second process makes
//! `clio backup restore` refuse with exit code 2, leaving the target and the
//! key store byte-identical, and the same restore succeeds once that process
//! lets go. The in-process refusal test cannot demonstrate this: a guarantee
//! across processes needs a second process, so the holder here is spawned,
//! never a handle in this one.
//!
//! ## Owns
//! - That transcript's assertions
//! - The child lock-holder it spawns (self-exec with a hold, a marker
//!   handshake, a bounded lifetime, and kill-and-reap on drop)
//!
//! ## Does not own
//! - The store lock and its two-process behavior (see `clio-store`'s store
//!   lock and its own cross-process tests)
//! - The restore's refusal rules and exit codes (see `backup_restore_flow`
//!   and `backup_restore_cli`)
//! - The shared fixtures (see `backup_cli_fixtures` and the hand-built
//!   archive fixture)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::backup_cli_fixtures::{Scratch, key_store_path, populated_store, run_json_in};
use crate::backup_cli_fixtures_handmade::hand_built_archive;
use clio_store::{DekProvider, LocalDevKms, SqliteStore};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The env var that turns this test binary's child run into a lock holder.
const CHILD_ENV: &str = "CLIO_LOCK_E2E_CHILD";

/// The child test's exact name, used as the spawn filter.
const CHILD_TEST: &str =
    "backup_restore_cli::backup_restore_cli_lock_e2e_tests::child_holds_the_store";

/// How long the child waits for its parent before giving up on its own.
const CHILD_WAIT: Duration = Duration::from_secs(60);

/// The default target store inside `scratch`.
fn store_path(scratch: &Scratch) -> PathBuf {
    scratch.join("clio.db")
}

/// The child half of the handshake. Runs as an ordinary test with no env var
/// and returns immediately, so it is a harmless no-op in the normal suite;
/// spawned with `CHILD_ENV`, it opens the store, signals its parent, and
/// waits to be released before dropping the store.
#[test]
fn child_holds_the_store() {
    let Ok(brief) = std::env::var(CHILD_ENV) else {
        return;
    };
    let mut lines = brief.lines();
    let store = PathBuf::from(lines.next().expect("child store path"));
    let marker = PathBuf::from(lines.next().expect("child marker path"));
    let release = PathBuf::from(lines.next().expect("child release path"));
    let kms: Arc<dyn DekProvider> =
        Arc::new(LocalDevKms::open_persistent(key_store_path(&store)).expect("open the key store"));
    let held = SqliteStore::open(&store, kms).expect("hold the store open");
    std::fs::write(&marker, b"held").expect("signal held");
    let deadline = Instant::now() + CHILD_WAIT;
    while !release.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(held);
}

/// A spawned child that is killed and reaped even when the test panics.
struct Child(std::process::Child);

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Wait for a file to appear, with a deadline so a lost handshake fails
/// instead of hanging.
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

/// Release the child and wait for it to exit cleanly.
fn release_child(release: &Path, child: &mut Child) {
    std::fs::write(release, b"release").expect("signal release");
    let deadline = Instant::now() + CHILD_WAIT;
    loop {
        if let Some(status) = child.0.try_wait().expect("reap the child") {
            assert!(status.success(), "the child exited with {status}");
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the child did not exit after release"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Spawn a real second process that holds the store open, and wait for its
/// marker. The child's streams go to files inside `scratch` so a failure is
/// diagnosable, and the returned guard kills and reaps it even on panic.
fn spawn_holder(scratch: &Scratch) -> (Child, PathBuf) {
    let marker = scratch.join("child-held");
    let release = scratch.join("child-release");
    let brief = format!(
        "{}\n{}\n{}",
        store_path(scratch).display(),
        marker.display(),
        release.display()
    );
    let stdout = std::fs::File::create(scratch.join("child-stdout.log")).expect("child stdout");
    let stderr = std::fs::File::create(scratch.join("child-stderr.log")).expect("child stderr");
    let child = Child(
        Command::new(std::env::current_exe().expect("this test binary"))
            .args([CHILD_TEST, "--exact", "--nocapture"])
            .env(CHILD_ENV, &brief)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("spawn the second process"),
    );
    wait_for_file(&marker, Duration::from_secs(30));
    (child, release)
}

/// A store held open by a real second process makes the restore refuse, and
/// the same restore succeeds once that process lets go.
///
/// The refusal is not instant by design: the restore waits its bounded time
/// for the hold to go away before it refuses, so this test takes a few
/// seconds. Do not shorten that wait or sleep around it.
#[test]
fn a_restore_through_the_real_command_refuses_while_another_process_holds_the_store() {
    let scratch = Scratch::new("restore-lock-e2e");
    populated_store(&store_path(&scratch), 3, 1);
    let (id, _) = hand_built_archive(
        &scratch,
        &store_path(&scratch),
        "clio-20260927T000000Z-locke2e1",
        false,
        |_| (),
    );
    let keys = key_store_path(&store_path(&scratch));
    let (mut child, release) = spawn_holder(&scratch);
    // The bytes are read after the child holds, like the in-process refusal
    // test reads them after opening its handle: opening a store can change
    // its bytes, and the claim is that the refused restore changes nothing
    // from the state the holder left behind.
    let keys_before = std::fs::read(&keys).expect("read the live key store");
    let target_before = std::fs::read(store_path(&scratch)).expect("read the target");

    let (code, _, err) = run_json_in(&scratch, &["restore", &id, "--yes"]);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(
        err.contains("another process holds it"),
        "the refusal names the contention: {err}"
    );
    assert!(
        err.contains("store lock"),
        "the refusal names the store lock: {err}"
    );
    // Naming a contending shared holder is best effort on Linux: the pid list
    // comes from `/proc/locks`, and the refusal is complete and correct when
    // the platform reports no holder (see `clio-store`'s store lock). What is
    // promised is that a named holder is a real one, so when the message names
    // holders they must be the child, the only process holding the store.
    #[cfg(target_os = "linux")]
    assert!(
        !err.contains("by pid(s) ") || err.contains(&format!("by pid(s) {}", child.0.id())),
        "a named holder is the process that holds the store: {err}"
    );
    assert_eq!(
        std::fs::read(&keys).expect("read the key store"),
        keys_before,
        "the key store is byte-identical after the refusal"
    );
    assert_eq!(
        std::fs::read(store_path(&scratch)).expect("read the target"),
        target_before,
        "the target is byte-identical after the refusal"
    );

    release_child(&release, &mut child);

    let (code, payload, err) = run_json_in(&scratch, &["restore", &id, "--yes"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(
        payload["result"],
        serde_json::json!("restored"),
        "{payload}"
    );
    assert_eq!(
        clio_store::sqlite_file_state(&store_path(&scratch))
            .expect("measure the restored target")
            .count_of("items"),
        Some(3),
        "the restored store holds the archive's items"
    );
}
