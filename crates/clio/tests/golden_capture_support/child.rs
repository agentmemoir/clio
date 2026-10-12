// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the capture's child-process boundary: the identity every
//! captured child is pinned to, the bounded run that reaps one child, and the
//! capture one command produces. No tests live here.
//!
//! ## Owns
//! - The child environment a capture pins
//! - The bounded child run and the drained streams it returns
//! - One captured command
//!
//! ## Does not own
//! - The fixture's file shape (`fixture`) or the scratch directory the
//!   file-writing tests use (`scratch`)
//! - The capture plan, the freshness guards, and the comparator (the parent
//!   module)
//! - Any assertion (`golden_capture_test`, `golden_repeatability_test`)
//!
//! ## Boundary
//!
//! Changes to how a child is pinned, bounded, or captured should normally
//! require changes here. Changes to what a capture is compared against should
//! not.
//!
//! Keep this module focused on one bounded child process.

// Capture-mode items run in the capture tool, not in a test binary.
#![allow(dead_code)]

use super::{CAPTURE_HOME, CHILD_DEADLINE, CaptureEntry};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Point one child at the capture's own identity: the caller's variables are
/// dropped, `HOME` is the fixed capture home so every rendered path is free of
/// the machine, and deployment-config loading is disabled so no config file on
/// the capture machine can decide the bytes.
///
/// Disabling the file is what makes the pin hold on every host. `HOME` alone
/// only names the path a config would live under, so a machine that happens to
/// hold one there — a container with that user, for instance — would resolve a
/// different database and the frozen capture would stop reproducing. An empty
/// `CLIO_DEPLOYMENT_CONFIG` is the documented way to disable loading.
pub fn pin_capture_env(command: &mut Command, home: &Path, columns: Option<&str>) {
    command.env_clear();
    command.env("HOME", home);
    command.env(clio_config::DEPLOYMENT_CONFIG_ENV, "");
    if let Some(width) = columns {
        command.env("COLUMNS", width);
    }
}

/// One finished child: its exit code and both drained streams.
pub(super) struct BoundedOutput {
    pub(super) code: Option<i32>,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
}

/// Read one piped stream to the end on its own thread, so a chatty child can
/// never block on a full pipe while the parent waits for its exit.
fn drain(pipe: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let (mut pipe, mut buffer) = (pipe, Vec::new());
        let _ = pipe.read_to_end(&mut buffer);
        buffer
    })
}

/// Run one child to completion with stdin null and both streams drained,
/// bounded by `deadline`: on the deadline the child is killed and reaped.
pub(super) fn run_bounded(
    command: &mut Command,
    deadline: Duration,
) -> Result<BoundedOutput, String> {
    let label = format!("{command:?}");
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not spawn {label}: {error}"))?;
    let stdout = drain(child.stdout.take().expect("stdout was piped"));
    let stderr = drain(child.stderr.take().expect("stderr was piped"));
    let deadline_at = Instant::now() + deadline;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline_at => thread::sleep(Duration::from_millis(10)),
            other => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout.join();
                let _ = stderr.join();
                return Err(match other {
                    Ok(_) => format!("{label} did not exit within {deadline:?} and was killed"),
                    Err(error) => format!("{label} could not be waited for: {error}"),
                });
            }
        }
    };
    let stdout = stdout
        .join()
        .map_err(|_| format!("{label} stdout read failed"))?;
    let stderr = stderr
        .join()
        .map_err(|_| format!("{label} stderr read failed"))?;
    Ok(BoundedOutput {
        code: status.code(),
        stdout,
        stderr,
    })
}

/// Capture one command's exact output from a bounded child of `binary`.
/// The child is pinned by [`pin_capture_env`], so its help output is identical
/// on every host and under every runner. Non-UTF-8 output is a refusal.
pub fn capture(
    binary: &Path,
    argv: &[String],
    columns: Option<&str>,
) -> Result<CaptureEntry, String> {
    let mut command = Command::new(binary);
    pin_capture_env(&mut command, Path::new(CAPTURE_HOME), columns);
    let output = run_bounded(command.args(argv), CHILD_DEADLINE)?;
    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| format!("`{}` wrote non-UTF-8 stdout: {error}", argv.join(" ")))?;
    let stderr = String::from_utf8(output.stderr)
        .map_err(|error| format!("`{}` wrote non-UTF-8 stderr: {error}", argv.join(" ")))?;
    Ok(CaptureEntry {
        argv: argv.to_vec(),
        exit: output.code,
        stdout,
        stderr,
    })
}
