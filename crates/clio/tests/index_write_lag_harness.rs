// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//!
//! # Responsibility
//!
//! This harness proves that a one-shot `clio remember` leaves its new item
//! searchable by the next command even when an older index backlog is larger
//! than one drain batch. It drives the compiled `clio` binary against Postgres,
//! so the guarantee holds for the command a user actually types.
//!
//! ## Owns
//! - The Postgres backlog larger than one bounded drain batch
//! - `remember`, `get`, then `recall` end to end over the real binary
//!
//! ## Does not own
//! - The bounded drain or its batch size (`clio-mcp` / `clio-index`)
//! - Admission scoring or retrieval fusion
//!
//! ## Boundary
//!
//! Changes to the one-shot write drain or its batch bound should normally
//! require changes here. Changes to scoring internals should not.
//!
//! Keep this harness focused on the write-then-recall lag.
//!
//! # Why it needs Postgres
//!
//! The backlog is seeded as real outbox rows in the same store the binary
//! opens, so the drain order and the batch bound are the ones production runs.
//! The harness skips when no Postgres answers.

mod common;

use clio::store::{DekProvider, LocalDevKms, PostgresStore, ScopedSchema, Store, lock_pg_test};
use clio::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Local Compose Postgres, the same URL the Makefile exports for parity.
const COMPOSE_PG: &str = "postgres://clio:clio@127.0.0.1:34310/clio";

/// Calls served so far in this test binary, so a unique name never repeats.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn unique(tag: &str) -> String {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("lag-{tag}-{}-{sequence}", std::process::id())
}

/// The Postgres URL to seed and drive: the parent `CLIO_DATABASE_URL` (the CI
/// service), falling back to local Compose.
fn pg_url() -> String {
    std::env::var("CLIO_DATABASE_URL").unwrap_or_else(|_| COMPOSE_PG.to_owned())
}

/// True when a Postgres server answers at `url`'s host and port within a short
/// budget, so the harness skips instead of failing where none is running.
fn postgres_reachable(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("postgres://")
        .or_else(|| url.strip_prefix("postgresql://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = match host_port.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().unwrap_or(5432)),
        None => (host_port, 5432),
    };
    let Ok(addresses) = (host, port).to_socket_addrs() else {
        return false;
    };
    addresses
        .into_iter()
        .any(|address| TcpStream::connect_timeout(&address, Duration::from_millis(500)).is_ok())
}

/// Unique temp directory for one scenario.
fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(unique(tag));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// The `clio` binary with a cleared environment, the seeded encryption key, and
/// the scenario's own key store so its DEKs never touch the host.
fn clio(kms_path: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_clio"));
    cmd.env_clear();
    if let Some(pattern) = std::env::var_os("LLVM_PROFILE_FILE") {
        cmd.env("LLVM_PROFILE_FILE", pattern);
    }
    cmd.env("CLIO_KMS_FILE", kms_path);
    common::seed_key(&mut cmd);
    cmd
}

fn run(kms_path: &Path, db: &str, args: &[&str]) -> Output {
    clio(kms_path)
        .args(args)
        .arg("--db")
        .arg(db)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn clio")
}

fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout was not JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

/// One indexable backlog item: an open item with dual content, so the drain
/// derives a lexical doc from it exactly as it would for a real write.
fn backlog_item(bank: &str, id: &str) -> MemoryItem {
    MemoryItem {
        id: id.to_owned(),
        bank_id: bank.to_owned(),
        subject_id: id.to_owned(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot: Some(json!({"entity": "backlog item"})),
        gist: Some(format!("backlog item {id}")),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    }
}

#[test]
fn a_remember_behind_a_full_backlog_is_recallable_by_the_next_command() {
    let base = pg_url();
    if !postgres_reachable(&base) {
        eprintln!("skip write-lag harness: no Postgres at the configured URL");
        return;
    }
    let _guard = lock_pg_test();
    let schema = ScopedSchema::new(&base);
    let db = schema.url().to_owned();
    let bank = unique("bank");
    let dir = temp_dir("write-lag");
    let kms_path = dir.join("clio.kms.json");

    // Seed an older backlog larger than one bounded drain batch, then drop the
    // seed store before the binary opens the schema. The backlog is sealed under
    // a process-local KMS, so the binary cannot read it and skips it: the point
    // is that the older rows still fill the bounded drain batch.
    let backlog = clio::index::DEFAULT_JOB_BATCH + 8;
    {
        let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
        let store = PostgresStore::open(&db, kms).expect("open seeding store");
        for i in 0..backlog {
            let id = format!("itm-lag-backlog-{i}");
            store
                .create_memory_item(&backlog_item(&bank, &id), "agent")
                .expect("seed backlog item");
        }
    }

    let fact = "Nadia keeps the amber sail on the top shelf";
    let stored = run(
        &kms_path,
        &db,
        &[
            "remember",
            fact,
            "--category",
            "persona",
            "--bank",
            &bank,
            "--output",
            "json",
        ],
    );
    assert_eq!(
        stored.status.code(),
        Some(0),
        "remember failed: {}",
        String::from_utf8_lossy(&stored.stderr)
    );
    let payload = json_stdout(&stored);
    assert_eq!(payload["pass"], json!(true), "{payload}");
    let id = payload["id"].as_str().expect("stored id").to_owned();

    // The item is durable, so it is readable by id even before the fix.
    let got = run(
        &kms_path,
        &db,
        &["get", &id, "--bank", &bank, "--output", "json"],
    );
    assert_eq!(
        got.status.code(),
        Some(0),
        "get failed: {}",
        String::from_utf8_lossy(&got.stderr)
    );

    // The next command must find the item: the write's own drain indexed it even
    // though the older backlog filled the bounded batch.
    let recalled = run(
        &kms_path,
        &db,
        &[
            "recall",
            "Nadia amber sail top shelf",
            "--bank",
            &bank,
            "--limit",
            "10",
            "--output",
            "json",
        ],
    );
    assert_eq!(
        recalled.status.code(),
        Some(0),
        "recall failed: {}",
        String::from_utf8_lossy(&recalled.stderr)
    );
    let hits = json_stdout(&recalled);
    let ids: Vec<&str> = hits["hits"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row["item_id"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        ids.contains(&id.as_str()),
        "the just-written item must be recallable by the next command; got {ids:?} from {hits}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
