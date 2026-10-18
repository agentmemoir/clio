// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for driving the real `clio` binary in the
//! cross-path compliance proof: the deployment salt and subject hashes the
//! journal denial and the member classification share, the cleared-environment
//! command with the scenario's key store, journal, backup directory, and salt,
//! and the bounded run that captures the exit and stderr of a refusal.
//!
//! ## Owns
//! - `salt_bytes`, `salt_text`, `subject_hash`, and `denied_hash`
//! - `clio_command`, `run_cli`, `CliOutput`, and `assert_refused`
//!
//! ## Does not own
//! - The seeded stores and the archive assembly (see `fixtures` and `support`)
//! - The scenarios and their assertions (see the `scenario_*` modules)
//!
//! ## Boundary
//!
//! Changes to how a CLI scenario is driven should normally require changes here.
//! Changes to a scenario's assertions should not.
//!
//! Keep this module focused on the CLI driver, with no tests.

use crate::fixtures::DENIED_SUBJECT;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The key seeding every spawned `clio` receives, re-exported so a scenario
/// points its own extra command at the same key file.
pub use crate::common::seed_key;

/// The generous bound every spawned `clio` is waited for: a refusal exits in
/// milliseconds, so this only turns a hang into a failure.
pub const CLI_WAIT: Duration = Duration::from_secs(120);

/// The one deployment salt the journal denial and the member classification
/// share, so a hashed reference matches the recorded denial.
pub fn salt_bytes() -> Vec<u8> {
    clio_compliance::tombstone_salt()
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

/// The exit and captured stderr of one finished `clio` run.
#[derive(Debug)]
pub struct CliOutput {
    /// The process exit code, or `None` when it was killed by a signal.
    pub code: Option<i32>,
    /// Everything the child wrote to stderr.
    pub stderr: String,
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
    seed_key(&mut cmd);
    cmd
}

/// Run `clio` with `args` to completion under a bounded wait, capturing its exit
/// and stderr. Both streams are drained on worker threads, so a child that
/// prints more than a pipe buffer holds still exits.
pub fn run_cli(journal: &Path, kms: &Path, backups: &Path, args: &[&str]) -> CliOutput {
    let mut child = clio_command(journal, kms, backups)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn clio");
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let stdout_reader = std::thread::spawn(move || drain(stdout));
    let stderr_reader = std::thread::spawn(move || drain(stderr));
    let deadline = Instant::now() + CLI_WAIT;
    let status = loop {
        match child.try_wait().expect("reap the clio child") {
            Some(status) => break status,
            None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            None => {
                let _ = child.kill();
                panic!("clio did not exit before the deadline");
            }
        }
    };
    let stderr = stderr_reader.join().unwrap_or_default();
    let _ = stdout_reader.join();
    CliOutput {
        code: status.code(),
        stderr,
    }
}

/// Read a child's stream to a string, discarding the result on error.
fn drain<R: Read>(mut reader: R) -> String {
    let mut text = String::new();
    let _ = reader.read_to_string(&mut text);
    text
}

/// Assert one CLI run refused with the named reason, as a usage refusal.
pub fn assert_refused(output: &CliOutput, needle: &str) {
    assert_eq!(
        output.code,
        Some(2),
        "the command must refuse (exit 2): {}",
        output.stderr
    );
    assert!(
        output.stderr.contains(needle),
        "the refusal must name {needle:?}: {}",
        output.stderr
    );
}
