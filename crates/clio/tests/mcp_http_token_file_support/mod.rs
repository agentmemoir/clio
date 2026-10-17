// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the harness the `clio mcp http` token-file
//! suites drive: the scratch credential store, the real `clio mcp http` child
//! with the port it announces, and the request and result helpers over one
//! `POST /mcp`.
//!
//! ## Owns
//! - `Store`: one owner-only credential store per test, removed on drop
//! - `clio`, `run_to_end`, and `Server`: the real binary, started and reaped
//! - `post`, `bearer`, `call`, `tool_code`, `forbidden`, `ok`, and the
//!   minted-token reader (`TOKEN_PREFIX`, `minted_token`)
//! - `digests_on_disk`, the read-back the leak assertions use
//!
//! ## Does not own
//! - What any test asserts about the product (`mcp_http_token_file_startup`
//!   and `mcp_http_token_file_binding` own those)
//! - The encryption-key seed (`tests/common`)
//! - The transport's own authorization rules (see `clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to how a server is started, reaped, or asked should normally
//! require changes here. Adding a scenario should not.
//!
//! Keep this module focused on launching and reading, not on asserting.
//!
//! # Isolation
//!
//! Every server binds `127.0.0.1:0` and the caller reads the assigned port back
//! from the line the server announces on its stderr, so no two servers collide.
//! Every store is a fresh scratch path under the OS temp directory carrying the
//! pid, a monotonic counter, and a nanosecond stamp, and is removed when the
//! guard drops, so no two tests share a path. Secrets come from the store
//! writer's own generator, never from a literal. Every child is killed and
//! reaped, by `Server::stop` and again by its `Drop`, so a panic leaks no
//! process and no port.

// This module is a shared fixture: each suite that reuses it is its own test
// binary, so one binary uses the parts it needs and its neighbour uses the rest.
#![allow(dead_code)]

use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Calls served so far in this test binary, so no two tests share a scratch name.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One scratch store path, removed when the guard drops.
pub struct Store {
    pub path: PathBuf,
}

impl Store {
    /// A unique scratch path. Nothing is written until [`Store::write_raw`]
    /// or a real record writer runs.
    pub fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        Self {
            path: std::env::temp_dir().join(format!(
                "am-http-token-file-{tag}-{}-{nanos}-{sequence}.json",
                std::process::id()
            )),
        }
    }

    /// The scratch path as the argument the binary takes.
    pub fn path(&self) -> &str {
        self.path.to_str().expect("utf-8 scratch path")
    }

    /// Write bytes the store writer never produced, for the refusal cases that
    /// need a file the loader must reject.
    pub fn write_raw(&self, text: &str) {
        fs::write(&self.path, text).expect("write the raw store");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600)).expect("tighten");
        }
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// The `clio` binary with a cleared environment and the seeded key file.
pub fn clio() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_clio"));
    cmd.env_clear();
    super::common::seed_key(&mut cmd);
    if let Some(pattern) = std::env::var_os("LLVM_PROFILE_FILE") {
        cmd.env("LLVM_PROFILE_FILE", pattern);
    }
    cmd
}

/// Run one `mcp http` to completion (the refusal paths all exit on their own).
pub fn run_to_end(args: &[&str]) -> Output {
    clio()
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn clio mcp http")
}

/// One running server: the child, its announced port, and a live handle on
/// everything it has written to stderr.
///
/// The stderr reader must outlive the announcement. A drained-and-dropped pipe
/// would make the server's next diagnostic write fail, and `eprintln!` panics
/// on a failed write, so a server whose telemetry pipe closed at startup would
/// die on its first admitted write instead of answering.
pub struct Server {
    child: Child,
    pub port: u16,
    stderr: Arc<std::sync::Mutex<Vec<u8>>>,
}

impl Server {
    /// Start a server and wait for the port it announces. Port zero means the
    /// line never arrived and the caller must fail on that.
    pub fn start(args: &[&str]) -> Self {
        Self::start_with(args, &[])
    }

    /// The same, with extra environment entries on the child.
    ///
    /// `clio` clears the environment, so a value that must be *absent* needs
    /// nothing here and a value that must be *set* — including one set to the
    /// empty string, which is a different case from an absent one — needs this.
    /// The host's own environment is never inherited, so nothing about the
    /// machine this suite runs on can change the child's configuration.
    pub fn start_with(args: &[&str], env: &[(&str, &str)]) -> Self {
        let mut cmd = clio();
        for (key, value) in env {
            cmd.env(key, value);
        }
        let mut child = cmd
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn clio mcp http");
        let stderr = child.stderr.take().expect("stderr piped");
        let collected: Arc<std::sync::Mutex<Vec<u8>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&collected);
        let (tx, rx) = std::sync::mpsc::channel();
        // The thread keeps draining to EOF so the pipe never fills and the
        // server never blocks on a write.
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut announced = false;
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                if let Ok(mut buf) = sink.lock() {
                    buf.extend_from_slice(line.as_bytes());
                }
                if !announced && let Some(addr) = line.trim_end().strip_prefix(PREFIX) {
                    let port = addr
                        .rsplit(':')
                        .next()
                        .and_then(|p| p.parse().ok())
                        .unwrap_or(0u16);
                    announced = true;
                    let _ = tx.send(port);
                }
            }
            let _ = tx.send(0);
        });
        let port = rx.recv_timeout(Duration::from_secs(20)).unwrap_or(0);
        Self {
            child,
            port,
            stderr: collected,
        }
    }

    /// Everything the server wrote to stderr so far.
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr.lock().expect("stderr lock")).into_owned()
    }

    /// Kill the child, reap it, and return its stdout.
    pub fn stop(mut self) -> Vec<u8> {
        let _ = self.child.kill();
        let mut out = Vec::new();
        if let Some(mut stdout) = self.child.stdout.take() {
            let _ = stdout.read_to_end(&mut out);
        }
        let _ = self.child.wait();
        out
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The stderr prefix the transport announces its bound address with.
pub const PREFIX: &str = "mcp http listening on ";

/// The stderr prefix the transport prints its one-time minted bearer token with.
pub const TOKEN_PREFIX: &str = "mcp http auth token: ";

/// The one minted token a server printed, or `None` when it printed none.
pub fn minted_token(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .find_map(|line| line.strip_prefix(TOKEN_PREFIX))
        .map(str::to_owned)
}

/// One `POST /mcp` and the status plus body that came back.
///
/// The write side is half-closed once the request is sent, so the server's read
/// of the declared body completes on this client rather than waiting for a close
/// it will never see.
pub fn post(port: u16, body: &str, extra: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nContent-Length: {}\r\n{extra}\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).expect("write");
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut buf = Vec::new();
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.read_to_end(&mut buf);
    let pos = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(buf.len());
    let head = String::from_utf8_lossy(&buf[..pos]);
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (
        status,
        String::from_utf8_lossy(&buf[pos + 4..]).into_owned(),
    )
}

/// The `Authorization` header carrying `secret`.
pub fn bearer(secret: &str) -> String {
    format!("Authorization: Bearer {secret}\r\n")
}

/// One JSON-RPC `tools/call` frame for `tool` with `arguments`.
#[allow(clippy::needless_pass_by_value)]
pub fn call(tool: &str, arguments: Value) -> String {
    serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": tool, "arguments": arguments}
    })
    .to_string()
}

/// The error code inside a structured tool rejection, if the call got that far.
pub fn tool_code(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    value["result"]["structuredContent"]["code"]
        .as_str()
        .map(str::to_owned)
}

/// Whether the tool call was refused with the permission code.
pub fn forbidden(body: &str) -> bool {
    tool_code(body).as_deref() == Some("forbidden")
}

/// Every stored digest in a token store file, read from the file itself rather
/// than recomputed, so the assertion is against what a leaked copy would hold.
pub fn digests_on_disk(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .expect("store file")
        .lines()
        .filter_map(|line| line.split('"').nth(3))
        .filter(|value| value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_owned)
        .collect()
}

/// Whether the tool call reported success.
pub fn ok(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .is_ok_and(|v| v["result"]["structuredContent"]["ok"].as_bool() == Some(true))
}
