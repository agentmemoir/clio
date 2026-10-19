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
//! - The MCP frame writer, the response reader, and the child process reaper
//!
//! ## Does not own
//! - The deployment fixture and the claims the routes prove (see
//!   `scenario_review_export`)
//! - The export writer and the destination custody boundary (see
//!   `clio-compliance`)
//!
//! ## Boundary
//!
//! Changes to how a review export is invoked should normally require changes
//! here. Changes to what the export writes should not.
//!
//! Keep this module focused on the two delivery routes, with no tests.

use crate::cli::{CliOutput, clio_command, run_cli};
use crate::scenario_review_export::Target;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// The bounded wait for one MCP response or a clean server exit.
const MCP_WAIT: Duration = Duration::from_secs(30);

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
    thread::spawn(move || {
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
    let status = child.0.wait().expect("wait for clio mcp stdio");
    assert!(status.success(), "clio mcp stdio exited {status:?}");
}
