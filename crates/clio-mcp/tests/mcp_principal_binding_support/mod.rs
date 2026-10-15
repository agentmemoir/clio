// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared harness of the credential-binding
//! integration suites: the scratch credential store, the live server bound to an
//! OS-assigned port, and the request/response and result-reading helpers.
//!
//! ## Owns
//! - `Store`: one owner-only credential store per test, removed on drop
//! - `Journal`: one scratch erasure-journal path, removed on drop, for a
//!   scenario whose server confirms an erasure
//! - `spawn_server_with` / `spawn_server_with_journal` / `spawn_server` /
//!   `bound_server` / `bound_server_with_shared` / `bound_server_with_journal` /
//!   `bound_server_without_journal`: the real accept loop on `127.0.0.1:0`,
//!   with the deployment options a scenario needs
//! - `post`, `bearer`, and the result predicates (`tool_ok`, `forbidden`, ...)
//! - The request builders (`export_at`, `erase_at`) and the `stored` read-back
//!
//! ## Does not own
//! - Any scenario or assertion (the test binaries own those)
//! - The transport's behavior (see `clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to how a credential store is written or a request is framed should
//! normally require changes here. Adding a scenario should not.
//!
//! Keep this module focused on the harness, not on the rules under test.
//!
//! # Isolation
//!
//! Every server binds `127.0.0.1:0` and the caller reads the assigned port back,
//! so no two servers collide. Every store is a fresh scratch path under the OS
//! temp directory carrying the pid, a monotonic counter, and a nanosecond stamp,
//! and is removed when the guard drops, so no two tests share a path. Secrets
//! come from the store writer, never from a literal. A server either names its
//! own erasure-journal path in its effective document — which the erase boundary
//! now accepts only when it matches the deployment resolution — or names none
//! (`bound_server_without_journal`), so a served confirmed erasure never writes
//! the deployment's resolved journal file. Nothing reads the host environment.

use clio_mcp::http::HttpConfig;
use clio_mcp::http_auth::CredentialStore;
use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_types::Actor;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Calls served so far, so no two tests share a scratch name.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One scratch store path, removed when the guard drops.
pub struct Store {
    path: PathBuf,
}

impl Store {
    /// A unique scratch path. Nothing is written until [`Store::write`] runs.
    pub fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        Self {
            path: std::env::temp_dir().join(format!(
                "am-mcp-binding-{tag}-{}-{nanos}-{sequence}.json",
                std::process::id()
            )),
        }
    }

    /// Write the records through the real store writer, so the file is
    /// owner-only exactly as an operator's file would be. Each record's secret
    /// is generated here and appended to `secrets` in the same order.
    pub fn write(
        &self,
        records: &[(&str, Actor, &[&str])],
        secrets: &mut Vec<String>,
    ) -> CredentialStore {
        use clio_config::auth_tokens::{new_auth_token, save_auth_tokens};
        let mut built = Vec::new();
        for (label, actor, banks) in records {
            let banks: Vec<String> = banks.iter().map(|b| (*b).to_owned()).collect();
            let (record, printed) = new_auth_token(label, actor.as_str(), &banks).expect("record");
            secrets.push(printed);
            built.push(record);
        }
        save_auth_tokens(&self.path, &built).expect("save");
        tighten(&self.path);
        CredentialStore::load(&self.path).expect("load")
    }

    /// Every stored digest, read back from the file rather than recomputed, so
    /// a leak assertion is against what a copied file would actually hold.
    pub fn digests(&self) -> Vec<String> {
        std::fs::read_to_string(&self.path)
            .expect("store file")
            .lines()
            .filter_map(|line| line.split('"').nth(3))
            .filter(|value| value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit()))
            .map(str::to_owned)
            .collect()
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Restrict a store file to its owner, which the loader refuses without.
fn tighten(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).expect("tighten");
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Serve through the real `serve_with_listener` in a background thread, with the
/// caller's open options.
///
/// The options are a parameter so a scenario that needs a deployment-level
/// permission switched on — the shared bank surface is the only one today — can
/// ask for it instead of asserting only its absence. This server names its own
/// erasure-journal path in its document; because that path diverges from the
/// deployment resolution, a served confirmed erasure is refused fail-closed and
/// writes nothing. A scenario that performs a confirmed erasure over a shared
/// lock uses the deployment resolution (or a matching document) instead.
pub fn spawn_server_with(cfg: HttpConfig, options: &McpOpenOptions) -> u16 {
    spawn_server_document(
        cfg,
        options,
        &journal_document(&unique_journal_path("server")),
    )
}

/// Serve with `journal` as the state's document-named erasure journal; the
/// guard removes the file when it drops. Because `journal` diverges from the
/// deployment resolution, a confirmed erasure is refused fail-closed, so this
/// pins that no denial is written at the divergent path.
pub fn spawn_server_with_journal(
    cfg: HttpConfig,
    options: &McpOpenOptions,
    journal: &Journal,
) -> u16 {
    spawn_server_document(cfg, options, &journal.document())
}

/// Open one state against `document` and serve it in a background thread.
fn spawn_server_document(cfg: HttpConfig, options: &McpOpenOptions, document: &Value) -> u16 {
    let state = McpState::open_with_effective(options, document).expect("state");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        let _ = clio_mcp::http::serve_with_listener(state, cfg, listener);
    });
    port
}

/// A unique journal path under the OS temp dir; never a path another server or
/// test shares, and never the deployment's resolved location.
fn unique_journal_path(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "am-mcp-binding-journal-{tag}-{}-{nanos}-{sequence}.json",
        std::process::id()
    ))
}

/// The effective document that pins `path` as a state's erasure journal; the
/// key is `clio_config`'s `CLIO_ERASURE_JOURNAL_FILE` override, supplied by the
/// host that already resolved the document instead of by the environment.
fn journal_document(path: &Path) -> Value {
    json!({ "erasure_journal_file": path.to_string_lossy() })
}

/// One scratch erasure-journal path, removed when the guard drops. The filename
/// format keeps two tests apart even though a divergent document is refused
/// before anything is written.
pub struct Journal {
    path: PathBuf,
}

impl Journal {
    /// A unique path under the OS temp dir; the erase boundary refuses a
    /// divergent document, so nothing is written here.
    pub fn new(tag: &str) -> Self {
        Self {
            path: unique_journal_path(tag),
        }
    }

    /// The effective document naming this journal for one opened state.
    pub fn document(&self) -> Value {
        journal_document(&self.path)
    }

    /// The divergent path a refused erasure must not write.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Serve with the default open options and only the default bank chosen.
pub fn spawn_server(cfg: HttpConfig, default_bank: &str) -> u16 {
    spawn_server_with(
        cfg,
        &McpOpenOptions {
            default_bank: default_bank.into(),
            ..McpOpenOptions::default()
        },
    )
}

/// A record-scoped server. `default_bank` is chosen by the caller, so one
/// harness can prove both the inside-the-list and outside-the-list
/// omitted-bank cases without a second fixture.
pub fn bound_server(store: &CredentialStore, default_bank: &str) -> u16 {
    bound_server_with_shared(store, default_bank, false)
}

/// A record-scoped server whose shared-bank deployment permission is `enabled`.
///
/// The shared gate is two separate conditions — a deployment permission and a
/// credential whose list names the bank — so proving one half of it needs a
/// fixture that sets the other.
pub fn bound_server_with_shared(
    store: &CredentialStore,
    default_bank: &str,
    shared_bank_enabled: bool,
) -> u16 {
    spawn_server_with(
        HttpConfig {
            credentials: Some(Arc::new(store.clone())),
            ..HttpConfig::default()
        },
        &McpOpenOptions {
            default_bank: default_bank.into(),
            shared_bank_enabled,
            ..McpOpenOptions::default()
        },
    )
}

/// A record-scoped server, exactly as [`bound_server`], whose state names
/// `journal` as its erasure journal: the compliance verb that confirms an
/// erasure records its durable denial in this test's own file.
pub fn bound_server_with_journal(
    store: &CredentialStore,
    default_bank: &str,
    journal: &Journal,
) -> u16 {
    spawn_server_with_journal(
        HttpConfig {
            credentials: Some(Arc::new(store.clone())),
            ..HttpConfig::default()
        },
        &McpOpenOptions {
            default_bank: default_bank.into(),
            ..McpOpenOptions::default()
        },
        journal,
    )
}

/// A record-scoped server that names no journal: the erase boundary keeps its
/// deployment resolution, so an unauthorized erase is refused for
/// authorization before any journal is touched. Use it for the
/// authorization-only scenarios; a scenario that performs a confirmed erasure
/// must name its own journal through [`bound_server_with_journal`].
pub fn bound_server_without_journal(store: &CredentialStore, default_bank: &str) -> u16 {
    spawn_server_document(
        HttpConfig {
            credentials: Some(Arc::new(store.clone())),
            ..HttpConfig::default()
        },
        &McpOpenOptions {
            default_bank: default_bank.into(),
            ..McpOpenOptions::default()
        },
        &json!({}),
    )
}

/// One `POST /mcp` and the status plus body that came back.
///
/// The write side is half-closed once the request is sent, so the server's read
/// of the declared body completes rather than waiting for a close.
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
    let head = String::from_utf8_lossy(&buf[..pos.min(buf.len())]);
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let from = (pos + 4).min(buf.len());
    (status, String::from_utf8_lossy(&buf[from..]).into_owned())
}

/// The `Authorization` header carrying `secret`.
pub fn bearer(secret: &str) -> String {
    format!("Authorization: Bearer {secret}\r\n")
}

/// One JSON-RPC `tools/call` frame for `tool` with `arguments`.
#[allow(clippy::needless_pass_by_value)]
pub fn call(tool: &str, arguments: Value) -> String {
    json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": tool, "arguments": arguments}
    })
    .to_string()
}

/// An `export` call at `bank`, the compliance verb whose authorization depends
/// on the acting actor.
pub fn export_at(bank: &str) -> String {
    call("export", json!({"bank": bank, "destination": "/dev/null"}))
}

/// An `erase_request` at `bank`, the other actor-gated compliance verb.
pub fn erase_at(bank: &str) -> String {
    call(
        "erase_request",
        json!({
            "bank": bank, "subject_id": "s1",
            "legal_basis": "test", "request_id": "r1", "confirm": true
        }),
    )
}

/// The error code inside a structured tool rejection, if the call got that far.
pub fn tool_error_code(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    value["result"]["structuredContent"]["code"]
        .as_str()
        .map(str::to_owned)
}

/// Whether the tool call reported success.
pub fn tool_ok(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .is_ok_and(|v| v["result"]["structuredContent"]["ok"].as_bool() == Some(true))
}

/// Whether the tool call was refused with the permission code.
pub fn forbidden(body: &str) -> bool {
    tool_error_code(body).as_deref() == Some("forbidden")
}

/// Whether an item is present at `bank`, read back through the real `get` tool.
pub fn stored(port: u16, bank: &str, item_id: &str, auth: &str) -> bool {
    let body = call("get", json!({"bank": bank, "item_id": item_id}));
    let (_, response) = post(port, &body, auth);
    tool_ok(&response)
}

/// Assert that nothing in `text` carries secret or digest material.
pub fn assert_no_secret_material(text: &str, secrets: &[String], digests: &[String]) {
    for secret in secrets {
        assert!(
            !text.contains(secret.as_str()),
            "output leaked a secret: {text}"
        );
    }
    for digest in digests {
        assert!(
            !text.contains(digest.as_str()),
            "output leaked a digest: {text}"
        );
    }
}
