// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the real cross-path deployment a compliance
//! scenario drives and the real spawned `clio sync serve` peer that shares its
//! journal: the deployment paths and command driver, the peer's bound address
//! and lifecycle, the wire push and pull calls, and a read-only probe of the
//! store the peer writes.
//!
//! ## Owns
//! - `Deployment`, the deployment paths and the real `clio` command driver
//! - `Peer`, one spawned serve process with its logs, address, and lifecycle
//! - `Peer::push`, `Peer::pull`, `Peer::offline`, and `Peer::restart`
//! - `PeerProbe`, the read-only view of the peer's store and quarantine rows
//!
//! ## Does not own
//! - The synthetic fixtures and read-back probes (see `fixtures` and `support`)
//! - The incomplete-state scenarios (`scenario_refusals_incomplete`)
//! - The store, request ledger, and erasure primitives (see `clio-store` and
//!   `clio-compliance`)
//!
//! ## Boundary
//!
//! Changes to how a deployment is driven or a peer is spawned should normally
//! require changes here. Changes to what a scenario asserts should not.
//!
//! Keep this module focused on the real deployment and the real peer beside it.

use crate::cli::clio_command;
use crate::fixtures::{BANK, DENIED_SUBJECT};
use clio::AmError;
use clio::store::{Store, SyncStore};
use clio_sync::client::{SyncClientConfig, pull, push};
use rusqlite::Connection;
use serde_json::Value;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Output, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

/// The one bearer the spawned peer is configured with and every client here
/// presents, so one synthetic constant configures both sides.
pub const PEER_TOKEN: &str = "cross-path-peer-token";
/// How long a spawned peer may take to report its bound address. Generous
/// because the serve bootstrap runs before it binds.
const ANNOUNCE_BOUND: Duration = Duration::from_secs(90);
/// The process's own foreground TTL; the guard's kill is the primary cleanup,
/// and the TTL keeps a child left behind by a crash from outliving the run.
const SERVE_TTL_SECS: &str = "600";
/// The privileged actor and scope bank every request verb carries.
const ACTOR_TAIL: [&str; 4] = ["--actor", "harness", "--bank", BANK];

/// The paths one command run needs, plus the one shared deployment key file and
/// the private scratch directory the real peer is spawned into.
pub struct Deployment {
    pub journal: PathBuf,
    pub keys: PathBuf,
    pub kms: PathBuf,
    pub backups: PathBuf,
    pub db: String,
    pub peer_dir: PathBuf,
}

impl Deployment {
    /// Run one `clio` command against this deployment's store, capturing both
    /// streams so a refusal's envelope is readable.
    pub fn run(&self, args: &[&str]) -> Output {
        clio_command(&self.journal, &self.kms, &self.backups)
            .env("CLIO_KEY_FILE", &self.keys)
            .env("CLIO_KEY_FILE_ONLY", "1")
            .args(args)
            .args(["--db", self.db.as_str()])
            .stdin(Stdio::null())
            .output()
            .expect("spawn clio")
    }

    /// Run one confirmed erasure through the real command.
    pub fn retry(&self, request_id: &str) -> Output {
        let mut args = vec![
            "compliance",
            "request-retry",
            "--request-id",
            request_id,
            "--subject-id",
            DENIED_SUBJECT,
            "--legal-basis",
            "GDPR_ART_17",
            "--confirm",
            "--output",
            "json",
        ];
        args.extend(ACTOR_TAIL);
        self.run(&args)
    }

    /// Run one request verb that only needs the actor and bank tail.
    pub fn verb(&self, verb: &str, request_id: &str) -> Output {
        let mut args = vec![
            "compliance",
            verb,
            "--request-id",
            request_id,
            "--output",
            "json",
        ];
        args.extend(ACTOR_TAIL);
        self.run(&args)
    }

    /// Record one peer's deletion evidence, the disposition that settles its
    /// copy.
    pub fn disposition(&self, request_id: &str, peer_id: &str, proof_ref: &str) -> Output {
        let mut args = vec![
            "compliance",
            "peer-disposition",
            "--request-id",
            request_id,
            "--peer-id",
            peer_id,
            "--disposition",
            "removal_evidence",
            "--proof-ref",
            proof_ref,
            "--output",
            "json",
        ];
        args.extend(ACTOR_TAIL);
        self.run(&args)
    }
}

/// One spawned `clio sync serve` peer, killed and reaped when the guard drops.
///
/// The peer reports the address it bound on stdout, which the reader thread
/// reads back: a bound, non-empty address is the proof the process started and
/// is live. `offline` kills and reaps it; `restart` starts a new process over
/// the same journal and store, the offline peer's stale rejoin.
pub struct Peer {
    child: Option<Child>,
    addr: String,
    dir: PathBuf,
    db: PathBuf,
    journal: PathBuf,
    backups: PathBuf,
    banks: Vec<String>,
    stdout_log: PathBuf,
    stderr_log: PathBuf,
}

impl Peer {
    /// Start one SQLite-backed peer under `dir`, sharing the deployment's
    /// `journal` and backup directory, with its own store and key store, and
    /// serving `banks`.
    ///
    /// # Panics
    /// The binary cannot be spawned, or it never reports an address.
    #[must_use]
    pub fn start(dir: &Path, journal: &Path, backups: &Path, banks: &[&str]) -> Self {
        std::fs::create_dir_all(dir).expect("create the peer directory");
        let mut peer = Self {
            child: None,
            addr: String::new(),
            dir: dir.to_path_buf(),
            db: dir.join("peer.db"),
            journal: journal.to_path_buf(),
            backups: backups.to_path_buf(),
            banks: banks.iter().map(|bank| (*bank).to_owned()).collect(),
            stdout_log: dir.join("serve.stdout.log"),
            stderr_log: dir.join("serve.stderr.log"),
        };
        peer.spawn();
        peer
    }

    /// Spawn one serve process and read the address it bound.
    fn spawn(&mut self) {
        if let Some(mut old) = self.child.take() {
            let _ = old.kill();
            let _ = old.wait();
        }
        let credentials = write_credential_store(&self.dir, &self.banks);
        // The peer's store needs its own key store: a file-backed key store
        // seals each DEK under the process key, and this child's process key
        // differs from the in-process key the deployment fixture writes under.
        let kms = self.dir.join("clio.kms.json");
        let stderr = std::fs::File::create(&self.stderr_log).expect("create the peer stderr log");
        let mut command = clio_command(&self.journal, &kms, &self.backups);
        command
            .env(
                "CLIO_DEPLOYMENT_CONFIG",
                self.dir.join("no-such-deployment.json"),
            )
            .args(["sync", "serve", "--bind", "127.0.0.1:0", "--token-file"])
            .arg(&credentials)
            .args(["--banks", &self.banks.join(",")])
            .args(["--ttl-secs", SERVE_TTL_SECS, "--output", "json", "--db"])
            .arg(&self.db)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr));
        let mut child = command.spawn().expect("spawn clio sync serve");
        let addr = drain_stdout(&mut child, &self.stdout_log);
        assert!(
            !addr.is_empty(),
            "the peer never reported a bound address; log:\n{}",
            self.logs()
        );
        self.addr = addr;
        self.child = Some(child);
    }

    /// The endpoint a client connects to: a bare `host:port`.
    #[must_use]
    pub fn remote(&self) -> &str {
        &self.addr
    }

    /// Take the peer offline: kill and reap it so its endpoint answers nothing.
    /// The last address is kept so a client call still targets the dead
    /// endpoint rather than a blank one.
    pub fn offline(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Restart the peer from the same journal and store: the offline peer's
    /// stale rejoin, reading the denials recorded while it was down.
    pub fn restart(&mut self) {
        self.spawn();
    }

    /// Push `store`'s pending mutations for `banks` to this peer.
    ///
    /// # Errors
    /// Transport, auth, or transient storage failures.
    pub fn push<T: Store + SyncStore + 'static>(
        &self,
        store: &Arc<T>,
        device: &str,
        banks: &[&str],
    ) -> Result<(), AmError> {
        let cfg = client(Arc::clone(store), device, self.remote());
        push(&cfg, &owned(banks)).map(|_| ())
    }

    /// Pull `banks` from this peer into `store`.
    ///
    /// # Errors
    /// Transport, auth, or a refused bank.
    pub fn pull<T: Store + SyncStore + 'static>(
        &self,
        store: &Arc<T>,
        device: &str,
        banks: &[&str],
    ) -> Result<(), AmError> {
        let cfg = client(Arc::clone(store), device, self.remote());
        pull(&cfg, &owned(banks)).map(|_| ())
    }

    /// Both captured process logs, stdout first.
    #[must_use]
    pub fn logs(&self) -> String {
        let stdout = std::fs::read_to_string(&self.stdout_log).unwrap_or_default();
        let stderr = std::fs::read_to_string(&self.stderr_log).unwrap_or_default();
        format!("--- stdout ---\n{stdout}--- stderr ---\n{stderr}")
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Prove the peer answers a pull while live, take it offline, and prove the
/// same pull now fails: real liveness, then a real offline endpoint.
pub fn live_then_offline(peer: &mut Peer, scratch: &Path, bank: &str) {
    let reader_dir = scratch.join("reader");
    std::fs::create_dir_all(&reader_dir).expect("create the reader directory");
    let reader = Arc::new(crate::fixtures::open_sqlite(
        &reader_dir.join("reader.db"),
        &reader_dir.join("reader.kms.json"),
    ));
    peer.pull(&reader, "reader-1", &[bank])
        .expect("the live peer answers the wire");
    peer.offline();
    assert!(
        peer.pull(&reader, "reader-1", &[bank]).is_err(),
        "the offline peer refuses the wire"
    );
}

/// A read-only probe of the store a peer writes: its quarantined mutation
/// reasons and its per-subject item counts.
pub struct PeerProbe {
    db: PathBuf,
}

impl PeerProbe {
    /// A probe over the SQLite file `db`.
    #[must_use]
    pub fn new(db: &Path) -> Self {
        Self {
            db: db.to_path_buf(),
        }
    }

    /// Every quarantine reason the peer recorded, sorted by mutation id.
    ///
    /// # Panics
    /// The peer's database cannot be queried.
    #[must_use]
    pub fn dead_letter_reasons(&self) -> Vec<String> {
        let conn = Connection::open(&self.db).expect("open the peer database");
        conn.execute_batch("PRAGMA query_only = ON")
            .expect("make the peer query-only");
        let mut stmt = conn
            .prepare("select reason_code from sync_dead_letter order by sync_mutation_id")
            .expect("prepare the quarantine read");
        stmt.query_map([], |row| row.get::<_, String>(0))
            .expect("query the quarantine rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("quarantine rows")
    }

    /// The number of item rows the peer holds for one subject in one bank.
    ///
    /// # Panics
    /// The peer's database cannot be queried.
    #[must_use]
    pub fn subject_items(&self, bank: &str, subject: &str) -> i64 {
        let conn = Connection::open(&self.db).expect("open the peer database");
        conn.execute_batch("PRAGMA query_only = ON")
            .expect("make the peer query-only");
        conn.query_row(
            "select count(*) from items where bank_id = ?1 and subject_id = ?2",
            rusqlite::params![bank, subject],
            |row| row.get(0),
        )
        .expect("probe the peer's items")
    }
}

/// Write one owner-only credential store whose one record reaches `banks` and
/// whose secret digest is [`PEER_TOKEN`]'s, and return its path.
fn write_credential_store(dir: &Path, banks: &[String]) -> PathBuf {
    use clio_config::auth_tokens::{digest_hex, new_auth_token, save_auth_tokens};
    let (mut record, _) = new_auth_token("cross-path-peer", "harness", banks)
        .expect("build the peer credential record");
    record.secret_sha256 = digest_hex(PEER_TOKEN);
    let path = dir.join("auth_tokens.json");
    save_auth_tokens(&path, &[record]).expect("write the peer credential store");
    path
}

/// Drain a peer's stdout into its log file on a reader thread, send the first
/// parsed address back, and keep draining until the stream closes.
fn drain_stdout(child: &mut Child, log: &Path) -> String {
    let stdout = child.stdout.take().expect("peer stdout piped");
    let log = log.to_path_buf();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .expect("open the peer stdout log");
        let mut writer = BufWriter::new(file);
        let mut announced = false;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if !announced {
                announced = true;
                let addr = serde_json::from_str::<Value>(&line)
                    .ok()
                    .and_then(|payload| {
                        payload
                            .get("addr")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_default();
                let _ = tx.send(addr);
            }
            let _ = writeln!(writer, "{line}");
        }
        let _ = writer.flush();
    });
    rx.recv_timeout(ANNOUNCE_BOUND).unwrap_or_default()
}

/// The one sync client configuration every wire call here shares.
fn client<T: Store + SyncStore + 'static>(
    store: Arc<T>,
    device: &str,
    remote: &str,
) -> SyncClientConfig {
    SyncClientConfig {
        device_id: device.to_owned(),
        store: Arc::clone(&store) as Arc<dyn Store>,
        sync: store as Arc<dyn SyncStore>,
        remote: Some(remote.to_owned()),
        token: Some(PEER_TOKEN.to_owned()),
        cipher: Vec::new(),
    }
}

/// Owned bank strings for one client call.
fn owned(banks: &[&str]) -> Vec<String> {
    banks.iter().map(|bank| (*bank).to_owned()).collect()
}
