// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0

//! This module is responsible for one long-lived `clio mcp stdio` child and the
//! JSON-RPC conversation a harness holds with it.
//!
//! ## Owns
//! - `McpSession`: the child process, its pipes, a response reader thread, a
//!   stderr capture thread, and the `call` helper
//!
//! ## Does not own
//! - The store, the temporary directory, and the `clio` command builder
//!   (`consolidate_wave_support`)
//! - What any scenario asserts about a tool's answer
//!
//! ## Boundary
//!
//! Changes to how a harness talks to a live server belong here; changes to what
//! it asks belong in the harness.
//!
//! Test-only. Never compiled into production builds.

use super::{BANK, Fixture, RECV_TIMEOUT};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;

/// A live `clio mcp stdio` child, killed and reaped even on a panicking assert.
pub struct McpSession {
    server: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    responses: Receiver<String>,
    stderr: Arc<Mutex<String>>,
}

impl Drop for McpSession {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
    }
}

impl McpSession {
    /// Open a long-lived session over the fixture's store and handshake.
    ///
    /// The binding under test is the process lifetime: an MCP server outlives
    /// its wave, a one-shot CLI process does not.
    pub fn open(f: &Fixture) -> Self {
        let mut child = f
            .command_for(&["mcp", "stdio", "--bank", BANK])
            .args(["mcp", "stdio", "--bank", BANK])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn clio mcp stdio");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("child stdout");
        let stderr = child.stderr.take().expect("child stderr");

        let (sender, responses) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let captured = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&captured);
        thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                if let Ok(mut guard) = sink.lock() {
                    guard.push_str(&line);
                }
                line.clear();
            }
        });

        let mut session = Self {
            server: child,
            stdin,
            responses,
            stderr: captured,
        };
        session.send(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-11-25", "capabilities": {},
                       "clientInfo": {"name": "harness", "version": "0"}}
        }));
        session.send(&serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        let _ = session.recv(1);
        session
    }

    fn send(&mut self, frame: &Value) {
        let stdin = self.stdin.as_mut().expect("session stdin is open");
        writeln!(stdin, "{frame}").expect("write frame");
        stdin.flush().expect("flush frame");
    }

    fn recv(&self, id: i64) -> Value {
        let line = self.responses.recv_timeout(RECV_TIMEOUT).unwrap_or_else(|error| {
            let captured = self
                .stderr
                .lock()
                .map(|text| text.clone())
                .unwrap_or_default();
            panic!("no MCP response for id {id} within {RECV_TIMEOUT:?}: {error}; stderr: {captured}");
        });
        let parsed: Value = serde_json::from_str(&line).expect("JSON-RPC response");
        assert_eq!(
            parsed["id"],
            Value::from(id),
            "out-of-order response: {parsed}"
        );
        parsed
    }

    /// Call one tool and return the whole tool result, `isError` included.
    ///
    /// A scenario that expects a rejection reads this; a scenario that expects
    /// a success payload uses [`Self::call`].
    #[allow(clippy::needless_pass_by_value)]
    pub fn call_raw(&mut self, id: i64, name: &str, arguments: Value) -> Value {
        self.send(&serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": name, "arguments": arguments}
        }));
        self.recv(id)["result"].clone()
    }

    /// Call one tool and return its `structuredContent`.
    #[allow(clippy::needless_pass_by_value)]
    pub fn call(&mut self, id: i64, name: &str, arguments: Value) -> Value {
        let result = self.call_raw(id, name, arguments);
        assert_eq!(result["isError"], Value::Bool(false), "{result}");
        let structured = result["structuredContent"].clone();
        assert!(
            structured.is_object(),
            "tool response has no structuredContent: {result}"
        );
        structured
    }

    /// Close stdin, which is EOF for the session, and wait for a clean exit.
    pub fn finish(mut self) -> std::process::ExitStatus {
        drop(self.stdin.take());
        self.server.wait().expect("wait for clio mcp stdio")
    }
}
