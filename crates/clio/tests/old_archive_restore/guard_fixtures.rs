// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the fixtures the guarded-restore CLI
//! scenarios share: the deployment salt and subject hashes the journal denial
//! and the member classification both use, the file hash, the journal and
//! backup directories, the seeded store, the refusal assertions, the bounded
//! `clio` drivers, and the real `clio` binary driver.
//!
//! ## Owns
//! - `salt_text`, `subject_hash`, and `denied_hash`
//! - `file_sha256`, `journal_file`, `backup_dir`, and `seed_sqlite`
//! - `clio_command`, `spawn_restore`, `run_restore`, `RestoreChild`, `RestoreOutput`, and
//!   `assert_refused`
//! - `Refusal`, `DENIED_SUBJECT_REFUSAL`, and `assert_refused_exactly`
//! - `CliOutput` and `run_cli`
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
use clio::ErrorCode;
use clio_backup::manifest::ManifestAuthority;
use clio_compliance::{JournalState, load_journal, sha256_hex_bytes, tombstone_salt};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// The generous bound every spawned `clio` command is waited for: a refusal
/// exits in milliseconds, so this only turns a hang into a failure.
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

/// The exit, stdout, and stderr of one finished `clio` run.
#[derive(Debug)]
pub struct CliOutput {
    /// The process exit code, or `None` when it was killed by a signal.
    pub code: Option<i32>,
    /// Everything the child wrote to stdout.
    pub stdout: String,
    /// Everything the child wrote to stderr.
    pub stderr: String,
}

/// Run one real `clio` command with `args` to completion under a bounded wait,
/// capturing its exit, stdout, and stderr.
///
/// The wait is bounded: a command that does not exit before [`RESTORE_WAIT`] is
/// killed and reaped, and the run fails instead of hanging. Both streams are
/// drained on worker threads, so a child that prints more than a pipe buffer
/// holds still exits.
pub fn run_cli(journal: &Path, kms: &Path, backups: &Path, args: &[&str]) -> CliOutput {
    let mut guard = ChildGuard(
        clio_command(journal, kms, backups)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn clio"),
    );
    let stdout = guard.0.stdout.take().expect("stdout is piped");
    let stderr = guard.0.stderr.take().expect("stderr is piped");
    let stdout_reader = thread::spawn(move || drain(stdout));
    let stderr_reader = thread::spawn(move || drain(stderr));
    let status = wait_for_exit(&mut guard.0);
    let stdout = stdout_reader.join().expect("join the stdout reader");
    let stderr = stderr_reader.join().expect("join the stderr reader");
    CliOutput {
        code: status.code(),
        stdout,
        stderr,
    }
}

/// The spawned child, killed and reaped when this guard drops, so a panic
/// before the wait leaves no process behind.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Wait for the child to exit, killing and reaping it when it does not exit
/// before the deadline.
fn wait_for_exit(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + RESTORE_WAIT;
    loop {
        match child.try_wait().expect("reap the clio child") {
            Some(status) => return status,
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("clio did not exit before the deadline");
            }
        }
    }
}

/// Read a child's stream to a string, so a pipe nobody drains cannot fill.
fn drain<R: Read>(mut reader: R) -> String {
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .expect("read the clio stream");
    String::from_utf8_lossy(&bytes).into_owned()
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

/// One production refusal: the exit code it maps to, the machine-readable
/// error code it carries, and the whole refusal message.
pub struct Refusal {
    /// The process exit code the refusal maps to.
    pub exit: i32,
    /// The machine-readable error code the refusal carries.
    pub code: &'static str,
    /// The whole refusal message, compared by equality.
    pub message: &'static str,
}

/// The production's refusal when an archive member references a subject the
/// live authority denies.
///
/// The error code is the public enum's own string, so a rename of the code
/// fails this check here instead of leaving a copied string behind.
pub const DENIED_SUBJECT_REFUSAL: Refusal = Refusal {
    exit: 2,
    code: ErrorCode::Forbidden.as_str(),
    message: "an archive member references a currently denied subject",
};

/// Assert one CLI restore refused with the production's exact exit code, error
/// code, and message.
///
/// Every value is compared by equality, so a refusal that changes in any of
/// them fails this check instead of still matching on a fragment of the old
/// text.
pub fn assert_refused_exactly(output: &RestoreOutput, refusal: &Refusal) {
    assert_eq!(
        output.code,
        Some(refusal.exit),
        "the restore must refuse (exit {}): {}",
        refusal.exit,
        output.stderr
    );
    let envelope: Value = serde_json::from_str(&output.stderr).unwrap_or_else(|error| {
        panic!(
            "the refusal envelope is not JSON ({error}): {}",
            output.stderr
        )
    });
    assert_eq!(
        envelope["code"],
        json!(refusal.code),
        "the refusal's error code: {}",
        output.stderr
    );
    assert_eq!(
        envelope["message"],
        json!(refusal.message),
        "the refusal's message: {}",
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
    stderr_reader: Option<JoinHandle<std::io::Result<()>>>,
    stdout_reader: Option<JoinHandle<std::io::Result<()>>>,
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
            reader
                .join()
                .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
                .unwrap_or_else(|error| panic!("join the stderr reader: {error}"));
        }
        if let Some(reader) = self.stdout_reader.take() {
            reader
                .join()
                .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
                .unwrap_or_else(|error| panic!("join the stdout reader: {error}"));
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
        thread::spawn(move || -> std::io::Result<()> {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                let read = reader.read_line(&mut line)?;
                if read == 0 {
                    return Ok(());
                }
                if let Ok(mut text) = shared.lock() {
                    text.push_str(&line);
                }
                let _ = sender.send(std::mem::take(&mut line));
            }
        })
    };
    let stdout_reader = thread::spawn(move || -> std::io::Result<()> {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            let read = reader.read_line(&mut line)?;
            if read == 0 {
                return Ok(());
            }
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
