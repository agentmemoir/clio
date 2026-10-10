// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the long-running transports' reaction to a stdout reader
//! that goes away: the serve commands keep the ignored `SIGPIPE` disposition,
//! so a dropped reader is a recoverable write error rather than a signal that
//! ends the process.
//!
//! ## Owns
//! - Closed-stdout end-to-end tests for `mcp stdio` and `sync serve`
//!
//! ## Does not own
//! - The one-shot closed-stdout contract (`broken_pipe_harness`)
//! - The disposition helpers themselves (`sigpipe_tests`)
//!
//! ## Boundary
//!
//! Tests here break when a serve command's signal disposition changes; they
//! never change it.
//!
//! `SIGPIPE` is a Unix signal, so this whole file is Unix-only.

#![cfg(unix)]

mod common;

use std::io::{Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// The `clio` binary with a cleared environment plus the harness key and the
/// coverage pattern the other harnesses also set.
fn clio() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_clio"));
    cmd.env_clear();
    if let Some(pattern) = std::env::var_os("LLVM_PROFILE_FILE") {
        cmd.env("LLVM_PROFILE_FILE", pattern);
    }
    common::seed_key(&mut cmd);
    cmd
}

/// Wait for the child, bounded: everything this file spawns is expected to exit
/// on its own, so a wait with no deadline would turn a hang into a stuck suite.
fn wait_bounded(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("clio serve command did not exit within 30s");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Read a child's stderr after it has exited.
fn drain_stderr(child: &mut Child) -> String {
    let mut stderr = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr);
    }
    stderr
}

/// A dropped stdout reader must not kill the stdio transport by `SIGPIPE`.
///
/// The request is written only after the read end is closed, so the response
/// write has nowhere to go. With `SIG_DFL` the kernel would end the process at
/// that write, which is the regression this pins. The stderr check proves the
/// session started and reached the failed write, so a setup failure cannot pass
/// this test silently.
#[test]
fn a_dropped_stdout_reader_does_not_kill_the_stdio_server() {
    let mut child = clio()
        .args(["mcp", "stdio", "--db", "sqlite::memory:"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn clio mcp stdio");

    // Close the response pipe before the server can write a response.
    drop(child.stdout.take());
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        writeln!(
            stdin,
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\
             \"params\":{{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{{}}}}}}"
        )
        .expect("write the initialize request");
        stdin.flush().expect("flush the request");
    }
    // End of input after the request: the session has nothing left to serve.
    drop(child.stdin.take());

    let status = wait_bounded(&mut child);
    let stderr = drain_stderr(&mut child);

    assert_eq!(
        status.signal(),
        None,
        "a dropped stdout reader must not end the server with a signal; stderr: {stderr}"
    );
    assert!(
        stderr.contains("mcp stdio error"),
        "the session must reach the failed response write, not exit before serving; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "a closed stdout must be a recoverable write error, not a panic; stderr: {stderr}"
    );
}

/// The same contract for `sync serve`: its one report is written after the
/// reader is already gone, and the process must still exit on its own.
#[test]
fn a_dropped_stdout_reader_does_not_kill_sync_serve() {
    let mut child = clio()
        .args([
            "sync",
            "serve",
            "--dev",
            "--db",
            "sqlite::memory:",
            "--ttl-secs",
            "0",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn clio sync serve");

    drop(child.stdout.take());

    let status = wait_bounded(&mut child);
    let stderr = drain_stderr(&mut child);

    assert_eq!(
        status.signal(),
        None,
        "a dropped stdout reader must not end `sync serve` with a signal; stderr: {stderr}"
    );
    assert_eq!(
        status.code(),
        Some(0),
        "`sync serve` writes its report and exits when the reader is gone; stderr: {stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "a closed stdout must be a recoverable write error, not a panic; stderr: {stderr}"
    );
}
