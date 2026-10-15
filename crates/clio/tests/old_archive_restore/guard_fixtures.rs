// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the fixtures the guarded-restore CLI
//! scenarios share: the deployment salt and subject hashes the journal denial
//! and the member classification both use, the file hash, the journal and
//! backup directories, the seeded store, the refusal assertion, and the real
//! `clio` binary driver.
//!
//! ## Owns
//! - `salt_text`, `subject_hash`, and `denied_hash`
//! - `file_sha256`, `journal_file`, `backup_dir`, and `seed_sqlite`
//! - `clio_command`, `spawn_restore`, `run_restore`, `RestoreChild`, `RestoreOutput`, and
//!   `assert_refused`
//! - `current_authority`
//!
//! ## Does not own
//! - The archive assembly and read-back probes (see `support`)
//! - The scenarios and their assertions (see `guard_cli`, `guard_cli_erasure`,
//!   `guard_cli_pg`)
//!
//! ## Boundary
//!
//! Changes to how a CLI scenario is driven should normally require changes
//! here. Changes to a scenario's assertions should not.
//!
//! Keep this module focused on the shared CLI fixtures, with no tests.

use crate::support::{DENIED_SUBJECT, Scratch, open_sqlite, write_item};
use clio_backup::manifest::ManifestAuthority;
use clio_compliance::{JournalState, load_journal, sha256_hex_bytes, tombstone_salt};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// The generous bound every spawned `clio backup restore` is waited for: a
/// refusal exits in milliseconds, so this only turns a hang into a failure.
pub const RESTORE_WAIT: Duration = Duration::from_secs(120);

/// The one deployment salt the journal denial and the member classification
/// share, so a hashed reference matches the recorded denial.
pub fn salt_bytes() -> Vec<u8> {
    tombstone_salt()
}

/// [`salt_bytes`] as the raw text the child process receives, so the CLI's
/// classification hashes with the same salt this process derives hashes with.
pub fn salt_text() -> String {
    String::from_utf8(salt_bytes()).expect("the deployment salt is UTF-8")
}

/// The deployment-wide identity of `subject` under the shared salt.
pub fn subject_hash(subject: &str) -> String {
    clio::store::sha256_hex(&salt_bytes(), subject)
}

/// The denied subject's deployment-wide identity.
pub fn denied_hash() -> String {
    subject_hash(DENIED_SUBJECT)
}

/// The authority a current journal records, for the archive that names it.
pub fn current_authority(journal: &Path) -> ManifestAuthority {
    match load_journal(journal) {
        JournalState::Current(journal) => ManifestAuthority {
            deployment_id: journal.deployment_id,
            generation: journal.generation,
            salt_fingerprint: journal.salt_fingerprint,
        },
        other => panic!("expected a current journal: {other:?}"),
    }
}

/// The SHA-256 of a file's bytes.
pub fn file_sha256(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    sha256_hex_bytes(&bytes)
}

/// A journal path under the scratch root whose parent directory exists, so a
/// refusal that never writes still has a place the authority could live.
pub fn journal_file(scratch: &Scratch, relative: &str) -> PathBuf {
    let path = scratch.join(relative);
    std::fs::create_dir_all(path.parent().expect("the journal path has a parent"))
        .expect("create the journal directory");
    path
}

/// The backup directory a scenario's archives live in.
pub fn backup_dir(scratch: &Scratch) -> PathBuf {
    let path = scratch.join("backups");
    std::fs::create_dir_all(&path).expect("create the backup directory");
    path
}

/// Seed a SQLite deployment with the given `(item, subject, gist)` triples.
pub fn seed_sqlite(db: &Path, kms: &Path, items: &[(&str, &str, &str)]) {
    std::fs::create_dir_all(db.parent().expect("the db has a parent")).expect("create the db dir");
    let store = open_sqlite(db, kms);
    for (id, subject, gist) in items {
        write_item(&store, id, subject, gist);
    }
}

/// The real `clio` binary, with a cleared environment and the scenario's key
/// store, journal, backup directory, and deployment salt.
pub fn clio_command(journal: &Path, kms: &Path, backups: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_clio"));
    cmd.env_clear();
    if let Some(pattern) = std::env::var_os("LLVM_PROFILE_FILE") {
        cmd.env("LLVM_PROFILE_FILE", pattern);
    }
    cmd.env("CLIO_KMS_FILE", kms);
    cmd.env("CLIO_ERASURE_JOURNAL_FILE", journal);
    cmd.env("CLIO_COMPLIANCE_SALT", salt_text());
    cmd.env("CLIO_BACKUP_DIR", backups);
    crate::common::seed_key(&mut cmd);
    cmd
}

/// The exit and captured stderr of one finished `clio backup restore`.
#[derive(Debug)]
pub struct RestoreOutput {
    /// The process exit code, or `None` when it was killed by a signal.
    pub code: Option<i32>,
    /// Everything the child wrote to stderr.
    pub stderr: String,
}

/// Assert one CLI restore refused with the named reason, as a usage refusal.
pub fn assert_refused(output: &RestoreOutput, needle: &str) {
    assert_eq!(
        output.code,
        Some(2),
        "the restore must refuse (exit 2): {}",
        output.stderr
    );
    assert!(
        output.stderr.contains(needle),
        "the refusal must name {needle:?}: {}",
        output.stderr
    );
}

/// A spawned `clio backup restore` whose streams are drained on worker threads.
///
/// The threads let a caller wait for one stderr line the child prints before it
/// blocks on the journal lock, and let a bounded [`RestoreChild::finish`]
/// collect the exit without the child ever blocking on a full pipe. The child
/// is killed and reaped when this guard drops.
pub struct RestoreChild {
    child: Child,
    lines: Receiver<String>,
    stderr: Arc<Mutex<String>>,
    stderr_reader: Option<JoinHandle<()>>,
    stdout_reader: Option<JoinHandle<()>>,
}

impl RestoreChild {
    /// Wait, bounded, for one stderr line containing `needle`.
    pub fn await_line(&self, needle: &str, limit: Duration) {
        let deadline = Instant::now() + limit;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(remaining) {
                Ok(line) if line.contains(needle) => return,
                Ok(_) => {}
                Err(error) => {
                    panic!("the restore never printed {needle:?} before the deadline: {error}")
                }
            }
        }
    }

    /// Wait, bounded, for the child to exit; return its exit code and stderr.
    pub fn finish(mut self, limit: Duration) -> RestoreOutput {
        let deadline = Instant::now() + limit;
        let status = loop {
            match self.child.try_wait().expect("reap the restore child") {
                Some(status) => break status,
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
                None => {
                    let _ = self.child.kill();
                    panic!("clio backup restore did not exit before the deadline");
                }
            }
        };
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
        if let Some(reader) = self.stdout_reader.take() {
            let _ = reader.join();
        }
        let stderr = self
            .stderr
            .lock()
            .map(|text| text.clone())
            .unwrap_or_default();
        RestoreOutput {
            code: status.code(),
            stderr,
        }
    }
}

impl Drop for RestoreChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Spawn `clio backup restore` with the given selector and flags, capturing both
/// streams on worker threads. The caller passes the snapshot id positional, or
/// `--latest`, and answers the confirmation with `--yes`.
pub fn spawn_restore(
    journal: &Path,
    kms: &Path,
    backups: &Path,
    target_db: &str,
    selector: &[&str],
) -> RestoreChild {
    let mut child = clio_command(journal, kms, backups)
        .arg("backup")
        .arg("restore")
        .args(selector)
        .arg("--yes")
        .arg("--db")
        .arg(target_db)
        .arg("--output")
        .arg("json")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn clio backup restore");
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let shared = Arc::new(Mutex::new(String::new()));
    let (sender, lines) = mpsc::channel();
    let stderr_reader = {
        let shared = Arc::clone(&shared);
        thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|read| read > 0) {
                if let Ok(mut text) = shared.lock() {
                    text.push_str(&line);
                }
                let _ = sender.send(std::mem::take(&mut line));
            }
        })
    };
    let stdout_reader = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        while reader.read_line(&mut line).is_ok_and(|read| read > 0) {
            line.clear();
        }
    });
    RestoreChild {
        child,
        lines,
        stderr: shared,
        stderr_reader: Some(stderr_reader),
        stdout_reader: Some(stdout_reader),
    }
}

/// Run `clio backup restore` to completion under a bounded wait, capturing its
/// exit and stderr.
pub fn run_restore(
    journal: &Path,
    kms: &Path,
    backups: &Path,
    target_db: &str,
    selector: &[&str],
) -> RestoreOutput {
    spawn_restore(journal, kms, backups, target_db, selector).finish(RESTORE_WAIT)
}
