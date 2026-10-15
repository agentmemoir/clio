// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared fixture the consolidate
//! end-to-end harness drives: an isolated store, a way to run the real `clio`
//! binary and a live `clio mcp stdio` session against it, and reads of the
//! stored artifacts a wave leaves behind.
//!
//! ## Owns
//! - `Fixture`: the temp dir, the sqlite store, and the `clio` command builder
//! - `McpSession`, re-exported from `mcp_session`: one long-lived MCP child with
//!   a JSON-RPC call helper
//! - `json_stdout` / `distilled_count`, the payload readers the tests share
//! - Re-exports of the scenario readers (`readers`)
//!
//! ## Does not own
//! - What any test asserts (the harness module owns its scenarios)
//! - The encryption-key seed (`tests/common`)
//! - The hub algorithm and the `consolidate` tool (`clio-write`)
//!
//! ## Boundary
//!
//! Changes to how a scenario launches or reads the real binary should normally
//! require changes here. Changes to what a scenario asserts should not require
//! changes here.
//!
//! Keep this module focused on launching and reading, not on asserting.

// This module is a shared fixture: each harness that reuses it is its own test
// binary, so one binary uses the parts it needs and its neighbour uses the rest.
#![allow(dead_code)]

use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};
// Every harness that reuses this fixture is its own test binary, so each one
// compiles the whole module and calls only part of it. A method unused by this
// binary is used by its neighbour.
#[allow(dead_code)]
mod distilled_records;
#[allow(dead_code)]
mod mcp_session;
#[allow(dead_code)]
mod readers;

// Re-exported for the harnesses, which are separate test binaries: nothing in
// this module calls them, so the re-export reads as unused from here.
#[allow(unused_imports)]
pub use mcp_session::McpSession;

// Re-exported for the harnesses, which are separate test binaries: nothing in
// this module calls them, so the re-export reads as unused from here.
#[allow(unused_imports)]
pub use readers::{
    MOVED_PAST, claim_set, current_list, distilled_bank, entry_ids, evidence_of, hit_ids,
    suppressed_raw_ids,
};

/// Longest we wait for one MCP response before failing the test.
pub const RECV_TIMEOUT: Duration = Duration::from_secs(10);
/// Safety net for a background wave. It bounds the test; it does not
/// synchronize it. The wait is a poll on a condition the wave itself creates
/// (its own audit row), so the poll ends when the work is done, not when a
/// duration happened to elapse.
pub const WAVE_TIMEOUT: Duration = Duration::from_secs(20);

/// The bank every scenario consolidates.
pub const BANK: &str = "hubbank";
/// Items that form a hub at the *default* degree limit of 16: every node is
/// linked to every other, so each one has degree 17.
pub const HUB_ITEMS: usize = 18;
/// Items whose degree is 5: above a tuned limit of 1, below the default of 16.
pub const TUNABLE_ITEMS: usize = 6;

/// Directories handed out so far, so two scenarios never share one.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A unique directory for one scenario, removed when the fixture is dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
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
            "clio-consolidate-{tag}-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("scenario dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One isolated store and the temp directory that owns it.
pub struct Fixture {
    dir: TempDir,
    db: PathBuf,
}

impl Fixture {
    /// A fresh, empty store.
    pub fn new() -> Self {
        let dir = TempDir::new("fresh");
        let db = dir.0.join("store.db");
        Self { dir, db }
    }

    /// A second store holding a byte copy of this one, so two bindings can run
    /// over an identical bank and a comparison cannot drift by accident.
    ///
    /// Two files make up a store. The `.db` is checkpointed first, because
    /// copying it alone would silently drop every commit still sitting in the
    /// write-ahead log. The per-database key store is copied too: it is the
    /// sibling `<db>.kms.json`, and a copy without it opens fine but cannot
    /// decrypt a single subject, so the wave would fail on its first item.
    pub fn copy_of(&self) -> Self {
        self.copy_with_keystore(true)
    }

    /// A second store holding a byte copy of the database file alone, with no
    /// key store beside it.
    ///
    /// This is the shape a database restored without its key store has. It
    /// opens cleanly, reads its rows, and fails at the first encrypted read, so
    /// it is a real storage failure at a real entry point rather than a mock.
    pub fn copy_without_keystore(&self) -> Self {
        self.copy_with_keystore(false)
    }

    fn copy_with_keystore(&self, keystore: bool) -> Self {
        self.checkpoint();
        let dir = TempDir::new("copy");
        let db = dir.0.join("store.db");
        std::fs::copy(&self.db, &db).expect("copy the prepared store");
        if keystore {
            let source = self.keystore_path();
            if source.exists() {
                let name = source.file_name().expect("keystore name");
                std::fs::copy(&source, dir.0.join(name)).expect("copy the key store");
            }
        }
        Self { dir, db }
    }

    fn keystore_path(&self) -> PathBuf {
        let mut name = self.db.file_name().expect("db name").to_os_string();
        name.push(".kms.json");
        self.db.with_file_name(name)
    }

    fn checkpoint(&self) {
        let conn = rusqlite::Connection::open(&self.db).expect("open the store to checkpoint");
        conn.execute_batch("pragma wal_checkpoint(truncate);")
            .expect("checkpoint the write-ahead log");
    }

    /// The `clio` binary, with a cleared environment, the shared key seed, and
    /// this store selected.
    pub fn command(&self) -> Command {
        self.command_for(&[])
    }

    /// The launcher for a run with `args`. A scenario that names its own bank
    /// replaces the pin rather than repeating it, because a repeated flag is refused.
    pub fn command_for(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_clio"));
        cmd.env_clear();
        // Forward the coverage profile pattern so instrumented children write
        // into the gitignored target directory instead of the source tree.
        if let Some(pattern) = std::env::var_os("LLVM_PROFILE_FILE") {
            cmd.env("LLVM_PROFILE_FILE", pattern);
        }
        super::common::seed_key(&mut cmd);
        cmd.args(["--db"]).arg(&self.db);
        // Pin the bank so the seeded items and the wave are in the same one,
        // whatever the host environment happens to name.
        if !args
            .iter()
            .any(|arg| *arg == "--bank" || arg.starts_with("--bank="))
        {
            cmd.args(["--bank", BANK]);
        }
        // A path that does not exist, so a run never reads an operator's
        // deployment overlay. An empty value is not used: that disables
        // deployment loading entirely rather than pointing it at nothing.
        cmd.env(
            "CLIO_DEPLOYMENT_CONFIG",
            self.dir.0.join("absent-overlay.json"),
        );
        // The cleared environment leaves no journal fallback, so name this
        // fixture's own file: a confirmed erasure records its durable denial
        // inside the directory the fixture removes when it drops.
        cmd.env(
            "CLIO_ERASURE_JOURNAL_FILE",
            self.dir.0.join("erasure-journal.json"),
        );
        cmd
    }

    /// Run one `clio` verb and assert it exited zero.
    pub fn assert_ok(&self, args: &[&str]) {
        let out = self.output(args);
        assert!(
            out.status.success(),
            "`clio {}` failed: {}\n{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Run one `clio` verb and return its JSON payload.
    pub fn json(&self, args: &[&str]) -> Value {
        json_stdout(&self.output(args))
    }

    /// Run one `clio` verb and return the raw result.
    pub fn output(&self, args: &[&str]) -> Output {
        self.command_for(args)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .expect("spawn clio")
    }

    /// Store `count` items and co-activate them in one retrieve, which is what
    /// builds the `assoc_edge` graph the hub scan reads.
    pub fn seed_coactivated_bank(&self, count: usize) {
        self.seed_coactivated_bank_in(BANK, count);
    }

    /// [`Self::seed_coactivated_bank`] in a named bank.
    ///
    /// The bank is supplied in the leading-global position. The parser threads
    /// leading globals after the verb's own flags, so this bank wins over the
    /// one the fixture's launcher pins; a scenario that needs two banks seeds
    /// the second one here.
    pub fn seed_coactivated_bank_in(&self, bank: &str, count: usize) {
        for i in 1..=count {
            self.assert_ok(&[
                "--bank",
                bank,
                "remember",
                &format!("hub probe note {i} about kubernetes staging rollout"),
                "--category",
                "schema",
                "--actor",
                "agent",
                "--output",
                "json",
            ]);
        }
        let recalled = self.json(&[
            "--bank",
            bank,
            "recall",
            "kubernetes staging rollout",
            "--limit",
            &count.to_string(),
            "--output",
            "json",
        ]);
        assert_eq!(recalled["ok"], Value::Bool(true), "{recalled}");
        assert_eq!(
            recalled["fused_candidates"],
            Value::from(count),
            "{recalled}"
        );
    }

    /// Co-activation degree of the highest-degree node in the seeded bank.
    pub fn max_coactivation_degree(&self) -> i64 {
        self.rows(
            "select 'd', max(c) from (select count(*) c from assoc_edges
             where assoc_kind = 'coactivation' group by source_id)",
        )
        .first()
        .map_or(0, |(_, n)| *n)
    }

    /// Hub audit rows as `(operation, count)`, ordered by operation.
    pub fn hub_audit(&self) -> Vec<(String, i64)> {
        self.rows(
            "select operation, count(*) from audit_events
             where operation like 'hub_distill%' group by operation order by operation",
        )
    }

    /// Ids of every stored hub-distilled item, sorted.
    pub fn distilled_ids(&self) -> Vec<String> {
        self.rows("select id, 0 from items where id like 'hub-distill:%' order by id")
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    /// A connection that reads stored columns and cannot change them.
    ///
    /// Writable with `PRAGMA query_only` rather than `SQLITE_OPEN_READ_ONLY`,
    /// for the reason [`Fixture::rows`] gives. Every reader in this file goes
    /// through here so the reason is stated once.
    pub(super) fn reading_connection(&self) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open(&self.db).expect("open the store to read it");
        conn.execute_batch("PRAGMA query_only = 1;")
            .expect("make the reading connection query-only");
        conn
    }

    /// Read a two-column result straight from SQLite, so assertions are on
    /// stored columns rather than on anything a tool reports about itself.
    ///
    /// The connection is writable with `PRAGMA query_only` set, not opened
    /// `SQLITE_OPEN_READ_ONLY`. The store is read here while a child process that
    /// owns it may still be finishing a write, and a child that exits or is
    /// killed mid-transaction leaves a hot journal behind it. SQLite must roll a
    /// hot journal back before the first read, and a read-only pager cannot, so
    /// the open fails with `SQLITE_READONLY_ROLLBACK` (776) and the panic names
    /// `prepare` rather than anything the test did. `query_only` keeps the
    /// promise these assertions rely on, that this connection cannot change the
    /// store, while letting SQLite recover the journal it finds.
    pub fn rows(&self, sql: &str) -> Vec<(String, i64)> {
        let conn = self.reading_connection();
        let mut stmt = conn.prepare(sql).expect("prepare");
        stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
    }

    /// Wait for a background wave to record its own completion row.
    pub fn await_hub_wave(&self) -> Vec<(String, i64)> {
        let deadline = Instant::now() + WAVE_TIMEOUT;
        loop {
            let audit = self.hub_audit();
            if audit.iter().any(|(op, _)| op == "hub_distill_complete") {
                return audit;
            }
            assert!(
                Instant::now() < deadline,
                "no hub_distill_complete audit row within {WAVE_TIMEOUT:?}: {audit:?}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Parse one command's stdout as JSON, with both raw streams on failure.
pub fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not JSON: {error}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// Parse one command's stderr as JSON, with both raw streams on failure.
///
/// The CLI writes its error envelope to stderr and exits non-zero, so a failed
/// command has nothing on stdout.
pub fn json_stderr(output: &Output) -> Value {
    let text = String::from_utf8_lossy(&output.stderr);
    serde_json::from_str(text.trim()).unwrap_or_else(|error| {
        panic!(
            "stderr is not JSON: {error}\nstdout: {}\nstderr: {text}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

/// The count a `hub_distill:` summary reports as distilled.
///
/// The summary is comma separated (`eligible=1,distilled=1,...`), so the split
/// covers both a comma and whitespace rather than assuming one of them.
pub fn distilled_count(summary: &str) -> Option<u32> {
    summary
        .split([',', ' ', '\t'])
        .find_map(|field| field.strip_prefix("distilled="))
        .and_then(|n| n.parse().ok())
}

/// One named count from a `hub_distill:` summary, by its own field name.
///
/// The same split as [`distilled_count`], and a prefix match on the whole field
/// name, so `stale_source_removed` and `stale_source_changed` cannot be confused
/// with `stale` or with each other.
pub fn summary_count(summary: &str, field: &str) -> Option<u32> {
    let prefix = format!("{field}=");
    summary
        .split([',', ' ', '\t'])
        .find_map(|part| part.strip_prefix(prefix.as_str()))
        .and_then(|n| n.parse().ok())
}
