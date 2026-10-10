// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the zero-config default database path end to end: the
//! compiled `clio` binary is launched with a controlled environment and the
//! resolved database file is inspected on disk.
//!
//! ## Owns
//! - Cold-start creation and reopen durability at the XDG default
//! - `CLIO_DATA_DIR` and `XDG_DATA_HOME` overrides with auto-created parents
//! - Postgres scheme inference, `--db`/`--backend` precedence, volatile
//!   `sqlite::memory:`, and the fail-closed malformed-data-dir error
//! - Help output showing the resolved default without side effects
//!
//! ## Does not own
//! - The resolver unit tests (`clio-config::db_path_tests`)
//! - Config/CLI default agreement (`clio-config::config::db_agreement_tests`)
//! - Store internals (`clio-store`)
//!
//! ## Boundary
//!
//! Tests here break when CLI default resolution or store opening changes; they
//! never change the implementation.
//!
//! Keep this module focused on end-to-end default-path behavior.

mod common;

use clio::store::{DekProvider, LocalDevKms, SqliteStore, Store};
use clio::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far in this test binary, so `temp_dir` never returns a
/// directory another scenario already created.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Local Compose Postgres, same URL the Makefile exports for parity.
const COMPOSE_PG: &str = "postgres://clio:clio@127.0.0.1:34310/clio";

/// Postgres URL for tests that need a live server: honor the parent
/// `CLIO_DATABASE_URL` (CI service on :5432), falling back to local Compose.
fn pg_url() -> String {
    std::env::var("CLIO_DATABASE_URL").unwrap_or_else(|_| COMPOSE_PG.to_string())
}

/// Unique temp directory for one scenario.
fn temp_dir(tag: &str) -> PathBuf {
    // The counter is what makes this distinct: the process id is constant
    // inside one test binary, so clock-plus-pid alone repeats whenever two
    // calls land in the same tick, and two scenarios then share one
    // directory and read each other's database.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "am-{tag}-{}-{nanos}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// The `clio` binary with a cleared environment plus the supplied overrides.
fn clio_cmd(envs: &[(&str, &str)]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_clio"));
    cmd.env_clear();
    // The child is coverage-instrumented under `cargo llvm-cov`. `env_clear`
    // drops LLVM_PROFILE_FILE, so the child falls back to writing
    // `default_*.profraw` into its CWD (this crate's directory). Forward the
    // parent pattern (it contains `%p`) so each child dumps into the
    // gitignored `target/` profile directory instead of the source tree.
    if let Some(pattern) = std::env::var_os("LLVM_PROFILE_FILE") {
        cmd.env("LLVM_PROFILE_FILE", pattern);
    }
    for (key, value) in envs {
        cmd.env(key, value);
    }
    common::seed_key(&mut cmd);
    cmd
}

fn item(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: format!("subj-{id}"),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.6),
        admission_score: Some(0.5),
        source_ref: Some("ev".into()),
        snapshot: Some(json!({ "k": id })),
        gist: Some("default-path gist".into()),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    }
}

/// Write one row, drop the store, reopen, and read it back.
fn persist_round_trip(db: &Path, id: &str) {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    {
        let store = SqliteStore::open(db, Arc::clone(&kms)).expect("open for write");
        store
            .create_memory_item(&item(id, "default"), "agent")
            .expect("write row");
    }
    let reopened = SqliteStore::open(db, kms).expect("reopen");
    let got = reopened
        .get_memory_item("default", id)
        .expect("read row after reopen");
    assert!(got.is_some(), "row `{id}` did not survive reopen");
}

#[test]
fn t31_01_no_flags_persists_to_xdg_default() {
    let home = temp_dir("t31-01-home");
    let home_s = home.to_str().expect("utf8 home");

    let out = clio_cmd(&[("HOME", home_s)])
        .args(["mcp", "stdio"])
        .stdin(Stdio::null())
        .output()
        .expect("run clio mcp stdio");
    assert!(
        out.status.success(),
        "stdio failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let db = home.join(".local/share/clio/clio.db");
    assert!(db.is_file(), "expected default db at {}", db.display());
    persist_round_trip(&db, "t31-01");

    let reopen = clio_cmd(&[("HOME", home_s)])
        .args(["ops", "diagnose"])
        .output()
        .expect("run clio ops diagnose");
    assert!(
        reopen.status.success(),
        "restart against the default db failed: {}",
        String::from_utf8_lossy(&reopen.stderr)
    );

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn t31_02_am_data_dir_override_creates_parent() {
    let dir = temp_dir("t31-02");
    let data = dir.join("nested").join("data");
    let data_s = data.to_str().expect("utf8 data dir");

    let out = clio_cmd(&[("CLIO_DATA_DIR", data_s)])
        .args(["mcp", "stdio"])
        .stdin(Stdio::null())
        .output()
        .expect("run clio mcp stdio");
    assert!(out.status.success());
    assert!(data.join("clio.db").is_file());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn t31_03_xdg_data_home_override() {
    let dir = temp_dir("t31-03");
    let xdg = dir.join("xdg");
    let xdg_s = xdg.to_str().expect("utf8 xdg");

    let out = clio_cmd(&[("XDG_DATA_HOME", xdg_s)])
        .args(["mcp", "stdio"])
        .stdin(Stdio::null())
        .output()
        .expect("run clio mcp stdio");
    assert!(out.status.success());
    assert!(xdg.join("clio/clio.db").is_file());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn t31_04_database_url_infers_postgres_backend() {
    let home = temp_dir("t31-04-home");
    let pg = pg_url();
    let out = clio_cmd(&[
        ("CLIO_DATABASE_URL", pg.as_str()),
        ("HOME", home.to_str().unwrap()),
    ])
    .args(["ops", "diagnose"])
    .output()
    .expect("run clio ops diagnose");
    assert!(
        out.status.success(),
        "postgres diagnose failed: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !home.join(".local/share/clio/clio.db").exists(),
        "a sqlite file was created despite CLIO_DATABASE_URL"
    );

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn t31_05_db_flag_wins_over_env_url() {
    let out = clio_cmd(&[(
        "CLIO_DATABASE_URL",
        "postgres://bogus:bogus@127.0.0.1:1/nope",
    )])
    .args(["ops", "diagnose", "--db", "sqlite::memory:"])
    .output()
    .expect("run clio ops diagnose");
    assert!(
        out.status.success(),
        "explicit --db did not win: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn t31_06_explicit_backend_and_db_win() {
    let dir = temp_dir("t31-06");
    let db = dir.join("explicit.db");
    let out = clio_cmd(&[("CLIO_DATABASE_URL", COMPOSE_PG)])
        .args([
            "ops",
            "diagnose",
            "--backend",
            "sqlite",
            "--db",
            db.to_str().unwrap(),
        ])
        .output()
        .expect("run clio ops diagnose");
    assert!(out.status.success());
    assert!(db.is_file(), "explicit --db file was not created");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn t31_07_memory_db_stays_volatile() {
    let home = temp_dir("t31-07-home");
    let out = clio_cmd(&[("HOME", home.to_str().unwrap())])
        .args(["mcp", "stdio", "--db", "sqlite::memory:"])
        .stdin(Stdio::null())
        .output()
        .expect("run clio mcp stdio");
    assert!(out.status.success());
    assert!(!home.join(".local/share/clio/clio.db").exists());

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn t31_08_help_shows_default_without_opening_the_db() {
    let home = temp_dir("t31-08-home");
    let home_s = home.to_str().expect("utf8 home");
    let expected = format!("{home_s}/.local/share/clio/clio.db");

    for args in [
        vec!["mcp", "stdio", "--help"],
        vec!["mcp", "http", "--help"],
        vec!["help"],
    ] {
        let out = clio_cmd(&[("HOME", home_s)])
            .args(&args)
            .output()
            .expect("run am");
        assert!(out.status.success(), "`{args:?}` failed");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains(&expected),
            "`{args:?}` missing default: {stdout}"
        );
        assert!(
            stdout.contains("precedence:"),
            "`{args:?}` missing precedence"
        );
    }
    assert!(
        !home.join(".local/share/clio/clio.db").exists(),
        "help opened the database"
    );

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn t31_09_data_dir_that_is_a_file_fails_closed() {
    let dir = temp_dir("t31-09");
    let file = dir.join("not-a-dir");
    std::fs::write(&file, b"x").expect("write file");

    let out = clio_cmd(&[("CLIO_DATA_DIR", file.to_str().unwrap())])
        .args(["ops", "diagnose"])
        .output()
        .expect("run clio ops diagnose");
    assert!(
        !out.status.success(),
        "malformed CLIO_DATA_DIR did not fail"
    );
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains("not-a-dir"),
        "error should name the path: {text}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
