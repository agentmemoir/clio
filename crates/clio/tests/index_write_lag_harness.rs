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
//! The harness fails closed when no Postgres answers: the gate panics unless
//! `CLIO_SKIP_PG_TESTS` opted out, in which case it prints the credential-safe
//! note and returns.

mod common;

use clio::store::{DekProvider, LocalDevKms, PostgresStore, ScopedSchema, Store, lock_pg_test};
use clio::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

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

/// Seed `count` older outbox rows in `bank`, then drop the seed store before the
/// binary opens the schema. The backlog is sealed under a process-local KMS, so
/// the binary cannot read it and skips it: the point is that the older rows
/// still fill the bounded drain batch.
fn seed_backlog(db: &str, bank: &str, count: usize) {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = PostgresStore::open(db, kms).expect("open seeding store");
    for i in 0..count {
        let id = format!("itm-lag-backlog-{i}");
        store
            .create_memory_item(&backlog_item(bank, &id), "agent")
            .expect("seed backlog item");
    }
}

#[test]
fn a_remember_behind_a_full_backlog_is_recallable_by_the_next_command() {
    let base = pg_url();
    if !clio::store::pg_test_target::postgres_reachable(&base) {
        clio::store::pg_skip::pg_required_or_skip(
            "the write-lag harness",
            "no Postgres at the configured URL",
        );
        return;
    }
    let _guard = lock_pg_test();
    let schema = ScopedSchema::new(&base);
    let db = schema.url().to_owned();
    let bank = unique("bank");
    let dir = temp_dir("write-lag");
    let kms_path = dir.join("clio.kms.json");

    // Seed an older backlog larger than one bounded drain batch, then drop the
    // seed store before the binary opens the schema.
    let backlog = clio::index::DEFAULT_JOB_BATCH + 8;
    seed_backlog(&db, &bank, backlog);

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

#[test]
fn a_batch_behind_a_full_backlog_is_recallable_by_the_next_command() {
    // A multi-item write reports every id it created, so all of them must be
    // searchable by the next command, not only the first.
    let base = pg_url();
    if !clio::store::pg_test_target::postgres_reachable(&base) {
        clio::store::pg_skip::pg_required_or_skip(
            "the write-lag harness",
            "no Postgres at the configured URL",
        );
        return;
    }
    let _guard = lock_pg_test();
    let schema = ScopedSchema::new(&base);
    let db = schema.url().to_owned();
    let bank = unique("batch-bank");
    let dir = temp_dir("write-lag-batch");
    let kms_path = dir.join("clio.kms.json");

    let backlog = clio::index::DEFAULT_JOB_BATCH + 8;
    seed_backlog(&db, &bank, backlog);

    let shelves = ["top", "middle", "bottom"];
    let ids: Vec<String> = (1..=shelves.len())
        .map(|n| format!("itm-lag-batch-{n}"))
        .collect();
    let operations: Vec<Value> = ids
        .iter()
        .zip(shelves)
        .map(|(id, shelf)| {
            json!({
                "tool": "store",
                "params": {
                    "item": {"id": id, "gist": format!("Nadia keeps the amber sail on the {shelf} shelf")},
                    "category": "persona",
                    "epistemic_kind": "fact"
                }
            })
        })
        .collect();
    let ops_path = dir.join("ops.json");
    std::fs::write(&ops_path, json!({"operations": operations}).to_string())
        .expect("write ops file");

    let batched = run(
        &kms_path,
        &db,
        &[
            "batch",
            "--file",
            ops_path.to_str().expect("utf8 ops path"),
            "--bank",
            &bank,
            "--output",
            "json",
        ],
    );
    assert_eq!(
        batched.status.code(),
        Some(0),
        "batch failed: {}",
        String::from_utf8_lossy(&batched.stderr)
    );

    let recalled = run(
        &kms_path,
        &db,
        &[
            "recall",
            "Nadia amber sail shelf",
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
    let found: Vec<&str> = hits["hits"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row["item_id"].as_str())
                .collect()
        })
        .unwrap_or_default();
    for id in &ids {
        assert!(
            found.contains(&id.as_str()),
            "every item the batch wrote must be recallable by the next command; {id} is missing from {found:?} ({hits})"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
