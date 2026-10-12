// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the store lock's two-process tests: the
//! cross-process guarantee cannot be demonstrated inside one process, so each
//! test here runs a real second process that holds, probes, or dies, and the
//! parent observes the lock from its own process.
//!
//! ## Owns
//! - The two-process scenarios at the lock and swap level: two processes
//!   sharing the store, a restore refused against a held store, a dying
//!   holder, a stale lock file, and a refused open
//! - The child-process harness (self-exec with a hold, a marker handshake,
//!   and a bounded lifetime)
//!
//! ## Does not own
//! - The lock's unit behavior (`store_lock_tests`)
//! - The live-CLI transcript (`backup_restore_lock_e2e_tests` in `clio`)
//!
//! ## Boundary
//!
//! Keep this file focused on cross-process lock behavior.

use super::*;
use crate::dek::{DekProvider, LocalDevKms};
use crate::restore_swap::{SwapRequest, swap_sqlite_store};
use crate::sqlite::SqliteStore;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Arc;

/// The env var that turns this test binary's child run into a lock holder.
const CHILD_ENV: &str = "CLIO_STORE_LOCK_CHILD";

/// The child test's exact name, used as the spawn filter.
const CHILD_TEST: &str = "store_lock::process_tests::child_holds_the_store_lock";

/// How long a child waits for its parent before giving up on its own.
const CHILD_WAIT: Duration = Duration::from_secs(60);

/// The child process's brief: which store to hold, where to signal.
struct ChildBrief {
    store: PathBuf,
    marker: PathBuf,
    release: PathBuf,
    /// Where the child writes the outcome of a shared-hold probe, when one
    /// is asked for.
    probe: Option<PathBuf>,
}

/// The child half of the handshake. Runs as an ordinary test with no env and
/// returns immediately; spawned with `CHILD_ENV`, it takes a shared hold,
/// signals its parent, and waits to be released.
#[test]
fn child_holds_the_store_lock() {
    let Ok(brief) = std::env::var(CHILD_ENV) else {
        return;
    };
    let mut lines = brief.lines();
    let brief = ChildBrief {
        store: PathBuf::from(lines.next().expect("child store path")),
        marker: PathBuf::from(lines.next().expect("child marker path")),
        release: PathBuf::from(lines.next().expect("child release path")),
        probe: lines
            .next()
            .filter(|out| !out.is_empty())
            .map(PathBuf::from),
    };
    let probe = match SharedHold::acquire(&brief.store) {
        Ok(hold) => {
            if let Some(out) = &brief.probe {
                let _ = std::fs::write(out, b"held");
            }
            Some(hold)
        }
        Err(error) => {
            if let Some(out) = &brief.probe {
                let _ = std::fs::write(out, error.message.as_bytes());
                return;
            }
            panic!(
                "the child could not take its shared hold: {}",
                error.message
            );
        }
    };
    std::fs::write(&brief.marker, b"held").expect("signal held");
    let deadline = Instant::now() + CHILD_WAIT;
    while !brief.release.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(probe);
}

/// A spawned child that is killed and reaped even when the test panics.
struct Child(std::process::Child);

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A scratch directory removed when the guard drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        Self(crate::test_unique::scratch_dir(&format!(
            "clio-store-lock-proc-{tag}"
        )))
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Spawn a second process that holds the store shared, and wait for its
/// signal. With `probe`, the child instead reports the outcome of a shared
/// hold it tried to take, and signals by writing that outcome.
fn spawn_holder(store: &Path, scratch: &Scratch, probe: bool) -> (Child, PathBuf, Option<PathBuf>) {
    let marker = scratch.join("child-held");
    let release = scratch.join("child-release");
    let probe_path = probe.then(|| scratch.join("child-probe-out"));
    let brief = format!(
        "{}\n{}\n{}\n{}",
        store.display(),
        marker.display(),
        release.display(),
        probe_path
            .as_ref()
            .map_or(String::new(), |path| path.display().to_string())
    );
    let stdout = std::fs::File::create(scratch.join("child-stdout.log")).expect("child stdout");
    let stderr = std::fs::File::create(scratch.join("child-stderr.log")).expect("child stderr");
    let child = Command::new(std::env::current_exe().expect("this test binary"))
        .args([CHILD_TEST, "--exact", "--nocapture"])
        .env(CHILD_ENV, &brief)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .expect("spawn the second process");
    let signalled = probe_path.as_ref().unwrap_or(&marker).clone();
    wait_for_file(&signalled, Duration::from_secs(30));
    (Child(child), release, probe_path)
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

/// A real file-backed store, closed before the caller proceeds.
fn a_real_store(path: &Path) {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open(path, kms).expect("open a file-backed store");
    drop(store);
}

/// The bytes of a file, for the byte-identical assertions.
fn bytes(path: &Path) -> Vec<u8> {
    let mut file = std::fs::File::open(path).expect("open for bytes");
    let mut out = Vec::new();
    file.read_to_end(&mut out).expect("read for bytes");
    out
}

/// A staged payload beside `target`, copied from the closed store.
fn staged_payload(target: &Path, scratch: &Scratch) -> PathBuf {
    let staged = scratch.join("staged.db");
    std::fs::copy(target, &staged).expect("stage a copy of the store");
    staged
}

/// Two processes can each hold the store shared, and a `backup create` against
/// a store another process holds shared still goes through unchanged: the
/// snapshot's own open takes a shared hold, which a shared hold permits.
#[test]
fn two_processes_can_each_hold_the_store_shared() {
    let scratch = Scratch::new("two-shared");
    let store = scratch.join("clio.db");
    a_real_store(&store);
    let (mut child, release, _probe) = spawn_holder(&store, &scratch, false);
    SharedHold::acquire(&store).expect("this process's shared hold");
    // A snapshot target must be absent or empty, so this is a fresh path
    // rather than a copy of the store.
    let staged = scratch.join("backup-create-staged.db");
    crate::snapshot_sqlite_into(&store, &staged)
        .expect("a backup create against a store held shared must succeed unchanged");
    assert!(
        staged.is_file(),
        "the backup create must have written its staged payload"
    );
    release_child(&release, &mut child);
}

/// A restore refuses while another process holds the store, naming the lock it
/// contends on, and nothing on disk changes. Naming the holding process is
/// best effort (see `store_lock`), so that part is asserted only when the
/// platform reports a holder.
#[test]
fn a_restore_refuses_while_another_process_holds_the_store() {
    let scratch = Scratch::new("refuse-held");
    let target = scratch.join("clio.db");
    a_real_store(&target);
    let staged = staged_payload(&target, &scratch);
    let target_before = bytes(&target);
    let (mut child, release, _probe) = spawn_holder(&target, &scratch, false);

    let error = swap_sqlite_store(&SwapRequest {
        staged_payload: staged.clone(),
        target: target.clone(),
        key: None,
    })
    .expect_err("a restore must refuse while another process holds the store");

    assert!(
        error.message.contains("another process holds it"),
        "the refusal names the contention: {}",
        error.message
    );
    // Naming a contending shared holder is best effort: the pid list comes
    // from `/proc/locks`, and the refusal is complete when it names none. A
    // named list is the child, the only process holding the store.
    assert!(
        !error.message.contains("by pid(s) ")
            || error
                .message
                .contains(&format!("by pid(s) {};", child.0.id())),
        "a named holder list is exactly the process that holds the store: {}",
        error.message
    );
    assert_eq!(bytes(&target), target_before, "the target is untouched");
    assert!(
        !scratch.join("clio.db-wal").exists() && !scratch.join("clio.db-shm").exists(),
        "no sidecar was touched"
    );
    assert!(staged.exists(), "the staged payload was not consumed");

    release_child(&release, &mut child);
    swap_sqlite_store(&SwapRequest {
        staged_payload: staged,
        target,
        key: None,
    })
    .expect("the restore goes through once the hold is released");
}

/// The refusal must carry no store content: only paths and pids the operator
/// already had. The store is seeded with a recognisable marker that must not
/// appear in the message.
#[test]
fn the_refusal_carries_no_store_content() {
    const MARKER: &str = "clio-secret-marker-8f21c4";
    let scratch = Scratch::new("no-content");
    let target = scratch.join("clio.db");
    a_real_store(&target);
    // Write the marker into the store file so the content genuinely lives in
    // the bytes a refusal could have leaked.
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&target)
            .expect("open the store to seed it");
        write!(file, "{MARKER}").expect("seed the marker");
    }
    let staged = staged_payload(&target, &scratch);
    let (mut child, release, _probe) = spawn_holder(&target, &scratch, false);

    let error = swap_sqlite_store(&SwapRequest {
        staged_payload: staged,
        target: target.clone(),
        key: None,
    })
    .expect_err("the restore must refuse while another process holds the store");

    assert!(
        !error.message.contains(MARKER),
        "the refusal must not carry store content: {}",
        error.message
    );

    release_child(&release, &mut child);
}

/// The holding process dying releases the lock; the OS releases it, so the
/// restore proceeds without any cleanup by the dead process.
#[test]
fn the_lock_is_released_when_the_holding_process_dies() {
    let scratch = Scratch::new("holder-dies");
    let target = scratch.join("clio.db");
    a_real_store(&target);
    let staged = staged_payload(&target, &scratch);
    let (mut child, _release, _probe) = spawn_holder(&target, &scratch, false);

    child.0.kill().expect("kill the holding process");
    child.0.wait().expect("reap the killed child");

    let report = swap_sqlite_store(&SwapRequest {
        staged_payload: staged,
        target,
        key: None,
    })
    .expect("the restore proceeds after the holder died");
    assert!(report.bytes > 0, "the swap really placed a store");
}

/// A stale lock file left by a dead process does not block a restore.
#[test]
fn a_stale_lock_file_does_not_block_a_restore() {
    let scratch = Scratch::new("stale-lock");
    let target = scratch.join("clio.db");
    a_real_store(&target);
    let staged = staged_payload(&target, &scratch);
    std::fs::write(lock_file_path(&target), b"pid 999999\n").expect("write a stale lock file");

    swap_sqlite_store(&SwapRequest {
        staged_payload: staged,
        target,
        key: None,
    })
    .expect("a stale lock file must not block");
}

/// The open half of the failed-open scenario: a store open in a second
/// process is refused, with the restoring process named, while a restore
/// holds the store exclusively.
#[test]
fn a_store_open_in_another_process_is_refused_while_a_restore_holds_the_lock() {
    let scratch = Scratch::new("open-refused");
    let store = scratch.join("clio.db");
    std::fs::write(&store, b"store").expect("write a stand-in store file");
    let hold = ExclusiveHold::acquire(&store).expect("take the exclusive hold");
    let (mut child, release, probe) = spawn_holder(&store, &scratch, true);
    let probe = probe.expect("the child was asked to report its probe");

    let outcome = std::fs::read_to_string(&probe).expect("the child reported its probe");
    assert!(
        outcome.contains("a restore is replacing it"),
        "the open is refused with the restore named: {outcome}"
    );
    assert!(
        outcome.contains(&std::process::id().to_string()),
        "the restoring process's pid is named: {outcome}"
    );

    std::fs::write(&release, b"release").expect("signal release");
    drop(hold);
    child.0.wait().expect("reap the child");
}
