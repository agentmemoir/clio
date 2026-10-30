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
//! - `Peer::credential_store` and `CREDENTIAL_STORE_FILE`, the store the peer writes
//! - `PeerProbe`, the read-only view of the peer's store and quarantine rows
//!
//! ## Does not own
//! - The synthetic fixtures, read-back probes, and the CLI driver (see
//!   `case_fixtures`, `support_test_support`, and `cli_test_support`)
//! - The incomplete-state scenarios (`scenario_refusals_incomplete`)
//! - The store, request ledger, and erasure primitives (see `clio-store` and `clio-compliance`)
//!
//! ## Boundary
//!
//! Changes to how a deployment is driven or a peer is spawned should normally
//! require changes here. Changes to what a scenario asserts should not.
//!
//! Keep this module focused on the real deployment and the real peer beside it.

use crate::case_fixtures::{BANK, DENIED_SUBJECT};
use crate::cli_test_support::{clio_command, run_captured};
use clio::AmError;
use clio::store::{Store, SyncStore};
use clio_sync::client::{SyncClientConfig, pull, push};
use serde_json::Value;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Output, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

pub use crate::peer_probe_test_support::PeerProbe;

/// The one bearer the spawned peer is configured with and every client here
/// presents, so one synthetic constant configures both sides.
pub const PEER_TOKEN: &str = "cross-path-peer-token";
/// The credential store the peer writes and the production reads the peer set from.
pub const CREDENTIAL_STORE_FILE: &str = "auth_tokens.json";
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
    ///
    /// The wait is bounded: a command that does not exit before the driver's
    /// deadline is killed and reaped, and the run fails instead of hanging.
    pub fn run(&self, args: &[&str]) -> Output {
        let mut command = clio_command(&self.journal, &self.kms, &self.backups);
        command
            .env("CLIO_KEY_FILE", &self.keys)
            .env("CLIO_KEY_FILE_ONLY", "1")
            .args(args)
            .args(["--db", self.db.as_str()]);
        run_captured(&mut command)
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
/// the same journal and store, the offline peer's stale rejoin. The reader
/// thread is joined once the peer is gone, so no thread outlives the process
/// whose stream it reads.
pub struct Peer {
    child: Option<Child>,
    reader: Option<JoinHandle<()>>,
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
            reader: None,
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
        self.stop();
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
        // The peer serves with the same key this harness's clients seal under, so
        // it can resolve a sealed row's subject and refuse an erased one.
        command.env(
            "CLIO_SYNC_KEY_FILE",
            crate::sync_wire_support::peer::shared_key_file(&self.dir),
        );
        let mut child = command.spawn().expect("spawn clio sync serve");
        let (addr, reader) = drain_stdout(&mut child, &self.stdout_log);
        self.reader = Some(reader);
        self.child = Some(child);
        assert!(
            !addr.is_empty(),
            "the peer never reported a bound address; log:\n{}",
            self.logs()
        );
        self.addr = addr;
    }

    /// The endpoint a client connects to: a bare `host:port`.
    #[must_use]
    pub fn remote(&self) -> &str {
        &self.addr
    }

    /// The credential store this peer wrote: the production derives the peer set from it.
    #[must_use]
    pub fn credential_store(&self) -> PathBuf {
        self.dir.join(CREDENTIAL_STORE_FILE)
    }

    /// Take the peer offline: kill and reap it so its endpoint answers nothing.
    /// The last address is kept so a client call still targets the dead
    /// endpoint rather than a blank one.
    pub fn offline(&mut self) {
        self.stop();
    }

    /// Kill and reap the peer, then join its stdout reader.
    ///
    /// The reader ends when the peer's stream closes, so the peer is killed
    /// first: joining a reader whose process still runs would wait for a
    /// process that never exits.
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(reader) = self.reader.take() {
            reader
                .join()
                .expect("the peer stdout reader thread panicked");
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
        self.stop();
    }
}

/// Prove the peer answers a pull while live, take it offline, and prove the
/// same pull now fails: real liveness, then a real offline endpoint.
pub fn live_then_offline(peer: &mut Peer, scratch: &Path, bank: &str) {
    let reader_dir = scratch.join("reader");
    std::fs::create_dir_all(&reader_dir).expect("create the reader directory");
    let reader = Arc::new(crate::case_fixtures::open_sqlite(
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

/// Write one owner-only credential store whose one record reaches `banks` and
/// whose secret digest is [`PEER_TOKEN`]'s, and return its path.
fn write_credential_store(dir: &Path, banks: &[String]) -> PathBuf {
    use clio_config::auth_tokens::{digest_hex, new_auth_token, save_auth_tokens};
    let (mut record, _) = new_auth_token("cross-path-peer", "harness", banks)
        .expect("build the peer credential record");
    record.secret_sha256 = digest_hex(PEER_TOKEN);
    let path = dir.join(CREDENTIAL_STORE_FILE);
    save_auth_tokens(&path, &[record]).expect("write the peer credential store");
    path
}

/// Drain a peer's stdout into its log file on a reader thread, send the first
/// parsed address back, and keep draining until the stream closes.
///
/// The reader handle comes back with the address, so the caller joins it: a
/// reader thread that outlives its peer keeps writing into state a later test
/// reads.
fn drain_stdout(child: &mut Child, log: &Path) -> (String, JoinHandle<()>) {
    let stdout = child.stdout.take().expect("peer stdout piped");
    let log = log.to_path_buf();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
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
    let addr = rx.recv_timeout(ANNOUNCE_BOUND).unwrap_or_default();
    (addr, reader)
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
        cipher: crate::sync_wire_support::sync_cipher(),
    }
}

/// Owned bank strings for one client call.
fn owned(banks: &[&str]) -> Vec<String> {
    banks.iter().map(|bank| (*bank).to_owned()).collect()
}
