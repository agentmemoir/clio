// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the Mem0 external provider adapter:
//! dry-run no-write validation, real pull and store, external-id idempotency,
//! and admission gate rejection.
//!
//! ## Owns
//! - Unit and integration tests for `import_provider` (dry-run, pull, idempotency, admission)
//! - Test double `FakeMem0Server`
//!
//! ## Does not own
//! - Security and failure tests (`provider_adapter_edge_tests.rs`)
//! - Low-level HTTP transport tests (`clio-index/http_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use clio_types::{EpistemicKind, SourceType};
use serde_json::{Value, json};

pub(super) fn state() -> Arc<crate::runtime::McpState> {
    crate::runtime::McpState::open_with_effective(
        &crate::runtime::McpOpenOptions::default(),
        &serde_json::json!({}),
    )
    .expect("state")
}

#[allow(clippy::needless_pass_by_value)]
pub(super) fn call(state: &crate::runtime::McpState, name: &str, args: Value) -> Value {
    crate::write_tools::dispatch(state, name, &args)
}

pub(super) struct FakeMem0Server {
    pub url: String,
    shutdown: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    pub _requests: Arc<AtomicUsize>,
    pub last_auth: Arc<Mutex<Option<String>>>,
    pub _last_path: Arc<Mutex<Option<String>>>,
}

impl FakeMem0Server {
    pub(super) fn spawn_handler<F>(handler: F) -> Self
    where
        F: Fn(&str, Option<&str>) -> (u16, Vec<u8>) + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fake bind");
        let port = listener.local_addr().expect("addr").port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicUsize::new(0));
        let last_auth = Arc::new(Mutex::new(None));
        let last_path = Arc::new(Mutex::new(None));

        let s_shutdown = Arc::clone(&shutdown);
        let s_requests = Arc::clone(&requests);
        let s_last_auth = Arc::clone(&last_auth);
        let s_last_path = Arc::clone(&last_path);
        let handler = Arc::new(handler);

        let handle = std::thread::spawn(move || {
            listener.set_nonblocking(true).expect("nonblocking");
            loop {
                if s_shutdown.load(Ordering::SeqCst) {
                    return;
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).expect("blocking");
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                        let mut buf = [0u8; 4096];
                        if let Ok(n) = stream.read(&mut buf)
                            && n > 0
                        {
                            s_requests.fetch_add(1, Ordering::SeqCst);
                            let req_str = String::from_utf8_lossy(&buf[..n]);
                            let first_line = req_str.lines().next().unwrap_or("");
                            let path = first_line.split_whitespace().nth(1).unwrap_or("/");
                            *s_last_path.lock().unwrap() = Some(path.to_owned());

                            let auth = req_str.lines().find_map(|l| {
                                let lower = l.to_ascii_lowercase();
                                if lower.starts_with("authorization:") {
                                    Some(l["authorization:".len()..].trim().to_owned())
                                } else {
                                    None
                                }
                            });
                            *s_last_auth.lock().unwrap() = auth.clone();

                            let (code, body) = handler(path, auth.as_deref());
                            let status_msg = if code == 200 { "OK" } else { "Error" };
                            let resp = format!(
                                "HTTP/1.1 {code} {status_msg}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = stream.write_all(resp.as_bytes());
                            let _ = stream.write_all(&body);
                            let _ = stream.flush();
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => return,
                }
            }
        });

        Self {
            url: format!("http://127.0.0.1:{port}"),
            shutdown,
            handle: Some(handle),
            _requests: requests,
            last_auth,
            _last_path: last_path,
        }
    }

    #[allow(clippy::needless_pass_by_value)]
    pub(super) fn spawn_json(body: Value) -> Self {
        let bytes = serde_json::to_vec(&body).expect("json bytes");
        Self::spawn_handler(move |_, _| (200, bytes.clone()))
    }

    pub(super) fn spawn_status(status: u16, body: &str) -> Self {
        let bytes = body.as_bytes().to_vec();
        Self::spawn_handler(move |_, _| (status, bytes.clone()))
    }
}

impl Drop for FakeMem0Server {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[test]
fn t01_dry_run_against_provider_zero_writes() {
    let s = state();
    let server = FakeMem0Server::spawn_json(json!([
        {
            "id": "dry-1",
            "memory": "User prefers dark mode for IDE theme.",
            "categories": ["preferences"]
        },
        {
            "id": "dry-2",
            "memory": "Rust toolchain target is x86_64-unknown-linux-gnu.",
            "categories": ["task_spec"]
        }
    ]));

    let out = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "dry_run": true,
            "bank": "test-bank",
            "actor": "harness",
            "options": {
                "api_base_url": server.url
            }
        }),
    );

    assert_eq!(out["ok"], Value::Bool(true), "{out}");
    assert_eq!(out["writes_performed"], 0);
    let report = &out["report"];
    assert_eq!(report["dry_run"], Value::Bool(true));
    assert_eq!(report["applied"], Value::Bool(false));
    assert_eq!(report["would_create"], 2);
    assert_eq!(report["would_skip"], 0);

    // Verify zero writes in the store
    assert!(
        s.store
            .get_memory_item("test-bank", "item-mem0-dry-1")
            .unwrap()
            .is_none()
    );
    assert!(
        s.store
            .get_memory_item("test-bank", "item-mem0-dry-2")
            .unwrap()
            .is_none()
    );
}

#[test]
fn t02_real_pull_stores_items_with_provenance_and_defaults() {
    let s = state();
    let server = FakeMem0Server::spawn_json(json!([
        {
            "id": "pull-101",
            "memory": "User prefers concise answers without ceremony.",
            "user_id": "usr-42",
            "categories": ["preferences"]
        }
    ]));

    let out = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "dry_run": false,
            "bank": "test-bank",
            "actor": "harness",
            "credentials": {
                "api_key": "sk-test-mem0-key"
            },
            "options": {
                "api_base_url": server.url
            }
        }),
    );

    assert_eq!(out["ok"], Value::Bool(true), "{out}");
    assert_eq!(out["writes_performed"], 1);
    assert_eq!(out["report"]["would_create"], 1);
    assert_eq!(out["report"]["applied"], Value::Bool(true));

    // Verify stored item properties: provenance, epistemic_kind, source_type
    let stored = s
        .store
        .get_memory_item("test-bank", "item-mem0-pull-101")
        .expect("read store")
        .expect("item exists");
    assert_eq!(stored.source_ref.as_deref(), Some("provider:pull-101"));
    assert_eq!(stored.epistemic_kind, EpistemicKind::Belief);
    assert_eq!(stored.source_type, Some(SourceType::ThirdParty));
    assert_eq!(
        stored.gist.as_deref(),
        Some("User prefers concise answers without ceremony.")
    );
    assert_eq!(stored.bank_id, "test-bank");

    // Verify auth header on server
    assert_eq!(
        server.last_auth.lock().unwrap().as_deref(),
        Some("Token sk-test-mem0-key")
    );
}

#[test]
fn t03_rerun_same_import_is_idempotent_no_duplicates() {
    let s = state();
    let server = FakeMem0Server::spawn_json(json!([
        {
            "id": "idem-201",
            "memory": "PostgreSQL database port is 34310."
        }
    ]));

    // First run creates
    let first = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "dry_run": false,
            "bank": "test-bank",
            "actor": "harness",
            "options": {
                "api_base_url": server.url
            }
        }),
    );
    assert_eq!(first["ok"], Value::Bool(true));
    assert_eq!(first["report"]["would_create"], 1);
    assert_eq!(first["report"]["would_skip"], 0);

    // Second run skips
    let second = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "dry_run": false,
            "bank": "test-bank",
            "actor": "harness",
            "options": {
                "api_base_url": server.url
            }
        }),
    );
    assert_eq!(second["ok"], Value::Bool(true));
    assert_eq!(second["report"]["would_create"], 0);
    assert_eq!(second["report"]["would_skip"], 1);
    assert_eq!(second["writes_performed"], 0);
}

#[test]
fn t04_imported_item_failing_admission_rejected_and_logged() {
    let s = state();
    let server = FakeMem0Server::spawn_json(json!([
        {
            "id": "admit-pass",
            "memory": "User prefers dark mode.",
            "categories": ["persona"]
        },
        {
            "id": "admit-fail",
            "memory": "Some ungrounded random scratch thought.",
            "category": "invalid_category_xyz"
        }
    ]));

    let out = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "dry_run": false,
            "bank": "test-bank",
            "actor": "harness",
            "options": {
                "api_base_url": server.url
            }
        }),
    );

    assert_eq!(out["ok"], Value::Bool(true), "{out}");
    let report = &out["report"];
    assert_eq!(report["would_create"], 1);
    assert_eq!(report["would_reject"], 1);
    let samples = report["rejection_samples"]
        .as_array()
        .expect("rejection samples");
    assert!(!samples.is_empty(), "{samples:?}");
    assert!(
        samples
            .iter()
            .any(|s| s.as_str().unwrap().contains("whitelist"))
    );

    // Admitted item exists; rejected item does not exist
    assert!(
        s.store
            .get_memory_item("test-bank", "item-mem0-admit-pass")
            .unwrap()
            .is_some()
    );
    assert!(
        s.store
            .get_memory_item("test-bank", "item-mem0-admit-fail")
            .unwrap()
            .is_none()
    );
}

#[test]
fn mem0_duplicate_external_ids_dry_run_matches_live_counts() {
    let s = state();
    let server = FakeMem0Server::spawn_json(json!([
        {"id": "dup-301", "memory": "First copy of the repeated external memory."},
        {"id": "dup-301", "memory": "First copy of the repeated external memory."},
        {"id": "dup-302", "memory": "Distinct external memory."}
    ]));

    let dry = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "dry_run": true,
            "bank": "test-bank-dedup",
            "actor": "harness",
            "options": {
                "api_base_url": server.url
            }
        }),
    );
    assert_eq!(dry["ok"], Value::Bool(true), "{dry}");
    assert_eq!(dry["writes_performed"], 0);
    assert_eq!(dry["report"]["would_create"], 2);
    assert_eq!(dry["report"]["would_skip"], 1);

    let live = call(
        &s,
        "import_provider",
        json!({
            "provider": "mem0",
            "dry_run": false,
            "bank": "test-bank-dedup",
            "actor": "harness",
            "options": {
                "api_base_url": server.url
            }
        }),
    );
    assert_eq!(live["ok"], Value::Bool(true), "{live}");
    assert_eq!(live["report"]["would_create"], 2);
    assert_eq!(live["report"]["would_skip"], 1);
    assert_eq!(live["writes_performed"], 2);
}
