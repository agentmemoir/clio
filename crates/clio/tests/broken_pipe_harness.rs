// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the process reaction to a closed stdout: `clio` must end
//! quietly when its reader goes away, not panic and not exit 101.
//!
//! ## Owns
//! - The closed-stdout end-to-end test over the real binary
//!
//! ## Does not own
//! - In-process output shaping (`cli_output`)
//! - The verb dispatch (`main`)
//!
//! ## Boundary
//!
//! Tests here break when the process-level output contract changes; they never
//! change it.
//!
//! `SIGPIPE` is a Unix signal, so this whole file is Unix-only.

#![cfg(unix)]

use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};

/// A reader that disappears must end `clio` quietly.
///
/// `mcp schema-export` prints far more than a pipe buffer holds, so the child
/// cannot finish writing before the read end is closed and must attempt a write
/// on a pipe nobody reads. That is the condition Rust's ignored `SIGPIPE` turns
/// into an `EPIPE` error, which `println!` used to panic on; the fix restores
/// the default disposition, so the kernel ends the process with no message.
///
/// The test asserts the signal itself, not merely a non-zero exit, and it leans
/// on the output exceeding a pipe buffer: a write that finished before the read
/// end closed would exit 0 with no signal.
#[test]
fn a_closed_stdout_ends_the_process_quietly() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_clio"))
        .args(["mcp", "schema-export"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn clio");

    // Close the read end at once, so the writer cannot drain into the buffer.
    drop(child.stdout.take());

    let mut stderr = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr);
    }
    let status = child.wait().expect("wait for clio");

    // A signal death carries no exit code, so assert the signal itself: `SIGPIPE`
    // is the contract. The assertion doubles as the size guard, because a write
    // that finished before the read end closed would exit 0 with no signal.
    assert_eq!(
        status.signal(),
        Some(libc::SIGPIPE),
        "a closed stdout must end the write with SIGPIPE; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "a closed stdout must print no panic; stderr: {stderr}"
    );
}
