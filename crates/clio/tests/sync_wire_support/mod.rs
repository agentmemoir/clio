// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the spawned-binary plumbing the sync-serve and
//! encrypted-sync harnesses need: one node's scratch directory, the owner-only
//! credential store a spawned relay loads, a running `clio sync serve` child,
//! the command wrapper that clears the environment, and a reader for the relay's
//! own journal table. Shared by the harness modules, and holding no test of its
//! own.
//!
//! ## Owns
//! - `NodeDir`, the per-node scratch directory and its cleanup
//! - `write_credential_store`, the owner-only store the spawned relay loads
//! - `Relay`, the spawned serve process, its bound address, and its kill-on-drop
//! - `run`, `expect_ok`, and `json_stdout`, the command wrapper
//! - `relay_rows` and `JournalRow`, the relay's stored bytes
//! - The synthetic bank, token, cipher secret, and attribute name
//!
//! ## Does not own
//! - Any assertion (`sync_wire_confidentiality_harness`)
//! - Other spawned-binary plumbing (`common`, `consolidate_wave_support`)
//!
//! ## Boundary
//!
//! Changes to how the binary is spawned or how the relay's journal is read
//! should normally require changes here. Changes to what is asserted about the
//! stored row should not.
//!
//! Keep this module focused on the spawned relay and the nodes around it.

// This module is a shared fixture: each suite that reuses it is its own test
// binary, so one binary uses the parts it needs and its neighbour uses the rest.
#![allow(dead_code)]

pub mod peer;

use rusqlite::Connection;
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// The bank both nodes sync. The relay keeps bank scope in the clear, which the
/// relay-confidentiality requirements permit.
pub const BANK: &str = "bp";
/// The bearer token the relay requires. Synthetic, not a real credential.
pub const TOKEN: &str = "sync-harness-token";
/// The cipher secret both nodes hold: a fresh synthetic value per process. A
/// real key in a test file would put key material in the repository.
pub static SYNC_KEY: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| format!("sync-harness-{:x}", rand::random::<u128>()));

/// One relay row as the relay's own database holds it: every stored column.
pub struct JournalRow {
    /// The relay's stable idempotency key for the mutation event.
    pub sync_mutation_id: String,
    /// Bank isolation key.
    pub bank_id: String,
    /// Record kind.
    pub entity_kind: String,
    /// Wire identifier: the opaque handle for a caller-chosen kind.
    pub entity_id: String,
    /// Operation.
    pub op: String,
    /// Writer's transaction-time stamp.
    pub transaction_time: String,
    /// Origin device id.
    pub device_id: String,
    /// The content digest the relay compares in its apply guard.
    pub content_hash: String,
    /// Sealed content, absent when the row is plaintext.
    pub content_sealed: Option<String>,
    /// `1` when the content is client-encrypted.
    pub encrypted: i64,
    /// The origin admission receipt, absent when there was none.
    pub admission_receipt: Option<String>,
}

impl JournalRow {
    /// Every stored column joined, so one assertion covers the whole row.
    ///
    /// All eleven columns are here on purpose. A substring assertion over a
    /// subset cannot see a leak in an omitted column, and it cannot see a
    /// digest oracle at all, which is why the harness also hashes candidate
    /// preimages against the stored idempotency key.
    #[must_use]
    pub fn stored_bytes(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.sync_mutation_id,
            self.bank_id,
            self.entity_kind,
            self.entity_id,
            self.op,
            self.transaction_time,
            self.device_id,
            self.content_hash,
            self.content_sealed.as_deref().unwrap_or_default(),
            self.encrypted,
            self.admission_receipt.as_deref().unwrap_or_default(),
        )
    }

    /// Whether the identifier column holds a fixed-width lowercase hex value,
    /// the shape an opaque handle has and a delimited caller-chosen id does not.
    #[must_use]
    pub fn is_handle_shaped(&self) -> bool {
        self.entity_id.len() == 32
            && self
                .entity_id
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    }
}

/// One node's scratch directory, removed when the guard drops.
///
/// `tempfile` is not a workspace dependency, so this is the repository's own
/// fixture pattern: a unique path under the OS temp directory plus explicit
/// cleanup. The pid and a monotonic counter keep parallel tests apart, and the
/// nanos keep two runs apart. Each node needs its own directory because the
/// store writes a sealed-key sidecar beside its database, and reusing a
/// directory across keys makes a later run report a corrupt configuration.
pub struct NodeDir(PathBuf);

impl NodeDir {
    /// A fresh directory for one node or relay.
    #[must_use]
    pub fn new(tag: &str) -> Self {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "clio-sync-wire-{tag}-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("node directory");
        Self(path)
    }

    /// The directory's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// A path inside the directory, not created.
    #[must_use]
    pub fn db(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// Take ownership of an existing directory, so a second guard can keep the
    /// path alive without creating a second one.
    #[must_use]
    pub fn hold(path: PathBuf) -> Self {
        Self(path)
    }
}

impl Drop for NodeDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `clio` with a cleared environment and one node's scratch directory, so a run
/// never reads the machine's config, key store, or data directory.
///
/// Five variables are load-bearing. `CLIO_KEY_FILE` and `CLIO_KEY_FILE_ONLY`
/// keep each node's content encryption in its own scratch key file, because the
/// OS keychain can prompt for access and a machine holding both a keychain entry
/// and a key file makes a write fail with a two-key conflict. `CLIO_KMS_FILE`
/// keeps each node's local dev-KMS key store in the same scratch directory: a
/// Postgres-backed node has no database file to name a sibling for, so without
/// it the key store falls back to the machine's data directory and every node
/// on the machine would share one, sealed under whichever node's key file wrote
/// it first. `CLIO_ERASURE_JOURNAL_FILE` names the node's own erasure journal:
/// the cleared environment leaves no data directory for the fallback to
/// resolve, and a serve start without a journal path refuses to bind.
/// `CLIO_DEPLOYMENT_CONFIG` points at a path that does not exist, so the
/// deployment config is defaulted instead of read from the machine's real one,
/// which keeps the result a fact about this test rather than about this host.
#[must_use]
pub fn clio(node: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_clio"));
    cmd.env_clear();
    cmd.env("CLIO_BACKUP_DIR", node.join("backups"));
    cmd.env("CLIO_KEY_FILE", node.join("keys.key"));
    cmd.env("CLIO_KEY_FILE_ONLY", "1");
    cmd.env("CLIO_KMS_FILE", node.join("clio.kms.json"));
    cmd.env(
        "CLIO_ERASURE_JOURNAL_FILE",
        node.join("erasure-journal.json"),
    );
    cmd.env(
        "CLIO_DEPLOYMENT_CONFIG",
        node.join("no-such-deployment-overlay.json"),
    );
    if let Some(pattern) = std::env::var_os("LLVM_PROFILE_FILE") {
        cmd.env("LLVM_PROFILE_FILE", pattern);
    }
    cmd
}

/// Run one command to completion against a node's store, with piped streams.
pub fn run(node: &Path, db: &Path, args: &[&str]) -> Output {
    clio(node)
        .args(args)
        .arg("--db")
        .arg(db)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|e| panic!("spawn clio {}: {e}", args.join(" ")))
}

/// Assert a command succeeded, quoting both streams when it did not.
pub fn expect_ok(args: &[&str], output: &Output) {
    assert!(
        output.status.success(),
        "`clio {}` failed: {}\nstdout: {}\nstderr: {}",
        args.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// One command's stdout as JSON, with both raw streams on failure.
#[must_use]
pub fn json_stdout(args: &[&str], output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "`clio {}` stdout is not JSON: {e}\nstdout: {}\nstderr: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// Write one scratch, owner-only credential store for a node: a single record
/// naming [`BANK`] whose secret digest is [`TOKEN`]'s, and return its path.
///
/// The server takes no single-token input any more, so a spawned relay is given
/// `--token-file` pointing here. The file is written through the config crate's
/// own writer, which is atomic and leaves the file owner-only — the loader
/// refuses a store another user can read. The record's generated secret is
/// discarded: the record must name the bearer this harness's clients already
/// present, so one synthetic constant configures both sides.
///
/// The store lives inside `dir` and is removed with it on drop.
#[must_use]
pub fn write_credential_store(dir: &NodeDir) -> PathBuf {
    write_credential_store_for(dir, &[BANK])
}

/// Write one scratch, owner-only credential store whose record reaches `banks`,
/// whose secret digest is [`TOKEN`]'s, and return its path.
///
/// The multi-bank form: a harness whose scenario covers more than one bank
/// needs the same one synthetic bearer to reach every bank it serves, because
/// the server's own bank authorization reads the presented credential's
/// records. The store lives inside `dir` and is removed with it on drop.
#[must_use]
pub fn write_credential_store_for(dir: &NodeDir, banks: &[&str]) -> PathBuf {
    use clio_config::auth_tokens::{
        AUTH_TOKEN_FILE_NAME, digest_hex, new_auth_token, save_auth_tokens,
    };
    let owned: Vec<String> = banks.iter().map(|bank| (*bank).to_owned()).collect();
    let (mut record, _generated) = new_auth_token("sync-harness", "harness", &owned)
        .expect("build the harness credential record");
    record.secret_sha256 = digest_hex(TOKEN);
    let path = dir.path().join(AUTH_TOKEN_FILE_NAME);
    save_auth_tokens(&path, &[record]).expect("write the harness credential store");
    path
}

/// A spawned `clio sync serve`, killed and reaped when the guard drops.
///
/// The relay binds an ephemeral port and reports the address it got, so this
/// test chooses no port and none can collide with a parallel test. The TTL is
/// short enough that a child left behind by a crash cannot outlive the run.
pub struct Relay {
    child: Child,
    addr: String,
    /// Held so the relay's directory outlives the child process.
    _dir: NodeDir,
}

impl Relay {
    /// Start a relay over `dir`'s store — with the credential store this
    /// harness writes into `dir` — and read back the address it bound.
    ///
    /// # Panics
    /// The binary cannot be spawned, or it never reports an address.
    #[must_use]
    pub fn start(dir: &NodeDir, db: &Path) -> Self {
        let credentials = write_credential_store(dir);
        let mut child = clio(dir.path())
            .args(["sync", "serve", "--bind", "127.0.0.1:0"])
            .arg("--token-file")
            .arg(&credentials)
            .args(["--banks", BANK, "--ttl-secs", "120", "--output", "json"])
            .arg("--db")
            .arg(db)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn clio sync serve");
        let addr = announced_addr(&mut child);
        assert!(!addr.is_empty(), "the relay never reported a bound address");
        Self {
            child,
            addr,
            _dir: NodeDir::hold(dir.path().to_path_buf()),
        }
    }

    /// The endpoint the client connects to. A bare `host:port`, not a URL: the
    /// client opens a TCP stream and writes the request head itself.
    #[must_use]
    pub fn remote(&self) -> &str {
        &self.addr
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Read the relay's bound address from its first stdout line, with a bounded
/// wait. An empty string means the line never arrived.
fn announced_addr(child: &mut Child) -> String {
    let stdout = child.stdout.take().expect("relay stdout piped");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut first = String::new();
        let read = BufReader::new(stdout).read_line(&mut first);
        let addr = match (read, serde_json::from_str::<Value>(&first)) {
            // `sync serve` prints one JSON object naming its address, then
            // holds the process open for the resolved TTL.
            (Ok(n), Ok(payload)) if n > 0 => payload
                .get("addr")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            _ => String::new(),
        };
        let _ = tx.send(addr);
    });
    rx.recv_timeout(Duration::from_secs(60)).unwrap_or_default()
}

/// Every journal row the relay stored for this harness's bank, read from the
/// relay's own database file rather than through the client API.
///
/// # Panics
/// The database cannot be opened or the journal cannot be read.
#[must_use]
pub fn relay_rows(relay_db: &Path) -> Vec<JournalRow> {
    let conn = Connection::open(relay_db).expect("open the relay's database");
    let mut stmt = conn
        .prepare(
            "select sync_mutation_id, bank_id, entity_kind, entity_id, op, \
             transaction_time, device_id, content_hash, content_sealed, encrypted, \
             admission_receipt from sync_mutations where bank_id = ?1 order by seq",
        )
        .expect("prepare the journal read");
    let rows = stmt
        .query_map([BANK], |row| {
            Ok(JournalRow {
                sync_mutation_id: row.get(0)?,
                bank_id: row.get(1)?,
                entity_kind: row.get(2)?,
                entity_id: row.get(3)?,
                op: row.get(4)?,
                transaction_time: row.get(5)?,
                device_id: row.get(6)?,
                content_hash: row.get(7)?,
                content_sealed: row.get(8)?,
                encrypted: row.get(9)?,
                admission_receipt: row.get(10)?,
            })
        })
        .expect("query the journal");
    rows.collect::<Result<Vec<_>, _>>().expect("journal rows")
}
