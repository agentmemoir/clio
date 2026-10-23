// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for driving the real `clio` binary in the
//! cross-path compliance proof: the deployment salt and subject hashes the
//! journal denial and the member classification share, the cleared-environment
//! command with the scenario's key store, journal, backup directory, and salt,
//! and the bounded run that captures the exit and both streams of a refusal.
//!
//! ## Owns
//! - `salt_bytes`, `salt_text`, `subject_hash`, and `denied_hash`
//! - `clio_command`, `run_captured`, `run_cli`, `CliOutput`, `assert_refused`,
//!   and `assert_refusal`
//! - `Refusal` and the production refusals the CLI returns
//!
//! ## Does not own
//! - The seeded stores and the archive assembly (see `case_fixtures` and
//!   `support_test_support`)
//! - The scenarios and their assertions (see the `scenario_*` modules)
//!
//! ## Boundary
//!
//! Changes to how a CLI scenario is driven should normally require changes here.
//! Changes to a scenario's assertions should not.
//!
//! Keep this module focused on the CLI driver, with no tests.

use crate::case_fixtures::DENIED_SUBJECT;
use clio::ErrorCode;
use serde_json::{Value, json};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

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
    crate::common::seed_key(&mut cmd);
    cmd
}

/// Run `clio` with `args` to completion under a bounded wait, capturing its
/// exit and stderr. Both streams are drained on worker threads, so a child that
/// prints more than a pipe buffer holds still exits.
pub fn run_cli(journal: &Path, kms: &Path, backups: &Path, args: &[&str]) -> CliOutput {
    let mut command = clio_command(journal, kms, backups);
    command.args(args);
    let output = run_captured(&mut command);
    CliOutput {
        code: output.status.code(),
        stderr: String::from_utf8(output.stderr).expect("the clio stderr is UTF-8"),
    }
}

/// Run one `clio` command to completion under a bounded wait, capturing its
/// exit status and both output streams.
///
/// The wait is bounded: a command that does not exit before [`CLI_WAIT`] is
/// killed and reaped, and the run fails instead of hanging. Both streams are
/// drained on worker threads, so a child that prints more than a pipe buffer
/// holds still exits.
pub fn run_captured(command: &mut Command) -> Output {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn clio");
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let stdout_reader = std::thread::spawn(move || drain(stdout));
    let stderr_reader = std::thread::spawn(move || drain(stderr));
    let status = wait_for_exit(&mut child);
    let stdout = join_reader(stdout_reader, "stdout");
    let stderr = join_reader(stderr_reader, "stderr");
    Output {
        status,
        stdout,
        stderr,
    }
}

/// Wait for the child to exit, killing and reaping it when it does not exit
/// before the deadline.
fn wait_for_exit(child: &mut std::process::Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + CLI_WAIT;
    loop {
        match child.try_wait().expect("reap the clio child") {
            Some(status) => return status,
            None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("clio did not exit before the deadline");
            }
        }
    }
}

/// Join one reader thread, surfacing its read failure and its panic where the
/// stream was read, instead of leaving the cause to surface later as a parse
/// failure.
fn join_reader(handle: std::thread::JoinHandle<std::io::Result<Vec<u8>>>, stream: &str) -> Vec<u8> {
    match handle.join() {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(error)) => panic!("read the clio {stream}: {error}"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// Read a child's stream to bytes, reporting a read failure to the caller.
fn drain<R: Read>(mut reader: R) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    Ok(bytes)
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

/// The production's refusal when an archive member carries usable key material
/// for a subject the live authority denies.
///
/// The error code is the public enum's own string, so a rename of the code
/// fails this check here instead of leaving a copied string behind.
pub const DENIED_SUBJECT_REFUSAL: Refusal = Refusal {
    exit: 2,
    code: ErrorCode::Forbidden.as_str(),
    message: "an archive member carries usable key material for a currently denied subject",
};

/// The production's refusal when an archive member cannot be classified.
///
/// The error code is the public enum's own string, so a rename of the code
/// fails this check here instead of leaving a copied string behind.
pub const UNCLASSIFIED_MEMBER_REFUSAL: Refusal = Refusal {
    exit: 2,
    code: ErrorCode::ConfigCorrupt.as_str(),
    message: "an archive member cannot be classified",
};

/// The production's refusal when a one-bank confirmation meets a subject key
/// that another bank also holds: the coordination set changed after the
/// confirmation.
///
/// The error code is the public enum's own string, so a rename of the code
/// fails this check here instead of leaving a copied string behind.
pub const PARTIAL_BANK_REFUSAL: Refusal = Refusal {
    exit: 1,
    code: ErrorCode::Forbidden.as_str(),
    message: "the banks sharing the subject key changed after confirmation",
};

/// Assert one CLI run refused with the production's exact exit code, error
/// code, and message.
///
/// Every value is compared by equality, so a refusal that changes in any of
/// them fails this check instead of still matching on a fragment of the old
/// text.
pub fn assert_refused(output: &CliOutput, refusal: &Refusal) {
    assert_refusal(output.code, &output.stderr, refusal);
}

/// The same refusal check for a run whose exit code and stderr are already
/// captured.
pub fn assert_refusal(code: Option<i32>, stderr: &str, refusal: &Refusal) {
    assert_eq!(
        code,
        Some(refusal.exit),
        "the command must refuse (exit {}): {stderr}",
        refusal.exit
    );
    let envelope: Value = serde_json::from_str(stderr)
        .unwrap_or_else(|error| panic!("the refusal envelope is not JSON ({error}): {stderr}"));
    assert_eq!(
        envelope["code"],
        json!(refusal.code),
        "the refusal's error code: {stderr}"
    );
    assert_eq!(
        envelope["message"],
        json!(refusal.message),
        "the refusal's message: {stderr}"
    );
}
