// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the two public routes that deliver a review
//! export: the `clio compliance review-export` command and the `review_export`
//! tool over the MCP stdio transport. It owns how each route is invoked and
//! how its answer is read back.
//!
//! ## Owns
//! - The CLI review-export invocation and the MCP stdio exchange
//! - The MCP frame writer, the response reader and its bounded thread join,
//!   the bounded exit wait, and the child process reaper
//!
//! ## Does not own
//! - The deployment fixture and the claims the routes prove (see
//!   `scenario_review_export`)
//! - The CLI driver and the captured run it returns (see `cli_test_support`)
//! - The export writer and the destination custody boundary (see
//!   `clio-compliance`)
//!
//! ## Boundary
//!
//! Changes to how a review export is invoked should normally require changes
//! here. Changes to what the export writes should not.
//!
//! Keep this module focused on the two delivery routes, with no tests.

use crate::cli_test_support::{CliOutput, clio_command, run_cli};
use crate::scenario_review_export::Target;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// The bounded wait for one MCP response or a clean server exit.
const MCP_WAIT: Duration = Duration::from_secs(30);

/// The bounded wait for the stdout reader thread to finish.
const READER_JOIN: Duration = Duration::from_secs(10);

/// Run the real CLI review export to `destination`.
pub(crate) fn run_cli_export(target: &Target, destination: &Path) -> CliOutput {
    let out = destination.display().to_string();
    let mut args = vec!["compliance", "review-export", "--out", out.as_str()];
    args.extend(["--actor", "harness", "--limit", "1000", "--output", "json"]);
    args.extend(target.args());
    run_cli(&target.journal, &target.kms, &target.backups, &args)
}

/// Write one JSON-RPC frame and flush it.
fn mcp_write(stdin: &mut ChildStdin, frame: &Value) {
    writeln!(stdin, "{frame}").expect("write frame");
    stdin.flush().expect("flush frame");
}

/// Write one request frame and return its response, skipping other frames.
fn mcp_exchange(
    stdin: &mut ChildStdin,
    responses: &mpsc::Receiver<String>,
    id: i64,
    frame: &Value,
) -> Value {
    mcp_write(stdin, frame);
    loop {
        let line = responses
            .recv_timeout(MCP_WAIT)
            .unwrap_or_else(|error| panic!("no MCP response for id {id} in {MCP_WAIT:?}: {error}"));
        let parsed: Value = serde_json::from_str(&line).expect("JSON-RPC response");
        if parsed.get("id").and_then(Value::as_i64) == Some(id) {
            return parsed;
        }
    }
}

struct Reaper(Child);
impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Join the MCP stdout reader thread under a bounded wait.
///
/// The reader ends when the child's stdout closes, and the child has already
/// exited and been reaped, so the wait ends. The bound turns a pipe that stays
/// open into a failure instead of a hang.
fn join_reader(handle: thread::JoinHandle<()>) {
    let deadline = Instant::now() + READER_JOIN;
    while !handle.is_finished() {
        assert!(
            Instant::now() < deadline,
            "the MCP stdout reader thread did not finish within {READER_JOIN:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
    handle
        .join()
        .expect("the MCP stdout reader thread panicked");
}

/// Wait for the MCP server to exit after its stdin is closed, killing and
/// reaping it when it does not exit before the deadline.
///
/// The wait is bounded: a server that hangs is killed and reaped, and the run
/// fails instead of waiting forever. `None` reports that the deadline passed.
fn wait_for_exit(child: &mut Child) -> Option<ExitStatus> {
    let deadline = Instant::now() + MCP_WAIT;
    loop {
        match child.try_wait().expect("reap the clio mcp stdio child") {
            Some(status) => return Some(status),
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Run the real MCP stdio `review_export` tool to `destination`.
pub(crate) fn run_mcp_export(target: &Target, destination: &Path) {
    let mut command = clio_command(&target.journal, &target.kms, &target.backups);
    command
        .args(["mcp", "stdio"])
        .args(target.args())
        .stdin(Stdio::piped());
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = Reaper(command.spawn().expect("spawn clio mcp stdio"));
    let mut stdin = child.0.stdin.take().expect("mcp stdin");
    let stdout = child.0.stdout.take().expect("mcp stdout");
    let (sender, responses) = mpsc::channel::<String>();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-11-25", "capabilities": {},
                   "clientInfo": {"name": "cross-path", "version": "0"}}
    });
    mcp_exchange(&mut stdin, &responses, 1, &init);
    mcp_write(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    );
    let call = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "review_export",
                   "arguments": {"destination": destination.display().to_string(),
                                 "limit": 1000, "actor": "harness"}}
    });
    let result = mcp_exchange(&mut stdin, &responses, 2, &call)["result"].clone();
    assert_eq!(result["isError"], json!(false), "{result}");
    assert_eq!(result["structuredContent"]["ok"], json!(true), "{result}");
    drop(stdin);
    let exit = wait_for_exit(&mut child.0);
    join_reader(reader);
    let Some(status) = exit else {
        panic!("clio mcp stdio did not exit within {MCP_WAIT:?} and was killed");
    };
    assert!(status.success(), "clio mcp stdio exited {status:?}");
}
