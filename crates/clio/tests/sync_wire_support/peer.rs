// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one spawned `clio sync serve` peer process:
//! its scratch credential store, its bound address and device identity, its
//! captured process logs, and a bounded stop/resume for an offline window:
//!
//! ## Owns
//! - `ServedPeer`, one spawned serve process with its logs and identity
//! - `stop` and `resume`, the bounded offline-window controls
//! - `hold`, the keepalive slot a backend-owned resource (a private database
//!   schema) rides in
//! - `shared_key_file` and `shared_key_secret`, the one sync key file the peer
//!   process reads and the in-process clients seal under
//!
//! ## Does not own
//! - The scratch directory and the cleared command environment (`NodeDir`,
//!   `clio` in `sync_wire_support`)
//! - The credential store writer (`write_credential_store_for`)
//! - What any scenario asserts about the peer's journal or store
//!
//! ## Boundary
//!
//! Changes to how a peer process is spawned, logged, or stopped should normally
//! require changes here. Changes to what a scenario asserts should not.
//!
//! Keep this module focused on one spawned serve process.

use serde_json::Value;
use std::any::Any;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use super::{NodeDir, clio, write_credential_store_for};

/// How long a spawned peer may take to report its bound address. Generous
/// because a Postgres-backed peer runs its bootstrap before it binds; the
/// scenario bodies stay far below it.
const ANNOUNCE_BOUND: Duration = Duration::from_secs(90);
/// The process's own foreground TTL; the guard's kill is the primary cleanup,
/// and the TTL keeps a child left behind by a crash from outliving the run.
const SERVE_TTL_SECS: &str = "600";

/// The 32 raw bytes of the one sync key this harness derives its key from,
/// computed from the process's synthetic secret so no key material is fixed in
/// the repository.
fn shared_key_bytes() -> [u8; 32] {
    let hex = clio_store::sha256_hex(super::SYNC_KEY.as_bytes(), "sync-wire-shared-key");
    let mut bytes = [0_u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).expect("hex digest");
    }
    bytes
}

/// Write the shared key file at `path`, owner-only: the loader refuses a file
/// another account can read, so the mode comes from the call that creates it.
fn write_key_file(path: &Path) {
    let bytes = shared_key_bytes();
    let _ = std::fs::remove_file(path);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .expect("the shared sync key file can be created");
        file.write_all(&bytes)
            .expect("the shared sync key can be written");
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes).expect("the shared sync key can be written");
    }
}

/// Write the shared sync key file into `dir` and return its path. The spawned
/// peer reads it through `CLIO_SYNC_KEY_FILE`, which is what lets it resolve a
/// sealed row's subject instead of storing the row unread.
pub fn shared_key_file(dir: &Path) -> PathBuf {
    let path = dir.join("clio-sync.key");
    write_key_file(&path);
    path
}

/// The secret form of the shared key: the CLI's own reader turns the key file's
/// raw bytes into the base64 text a `SyncCipherKey` is derived from, so this one
/// key file gives the spawned peer and an in-process client the same key.
pub fn shared_key_secret() -> String {
    static SECRET: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SECRET
        .get_or_init(|| {
            let path = std::env::temp_dir().join(format!(
                "clio-sync-wire-{}-{:x}.key",
                std::process::id(),
                rand::random::<u32>()
            ));
            write_key_file(&path);
            let secret = clio_config::sync_key::read_key_file_base64(&path)
                .expect("the shared sync key file reads back as the CLI's secret form");
            let _ = std::fs::remove_file(&path);
            secret
        })
        .clone()
}

/// One spawned `clio sync serve` peer, killed and reaped when the guard drops.
///
/// Both streams are retained as process log files inside the peer's scratch
/// directory: stderr is the child's own file, stdout is drained by a reader
/// thread that also reads back the bound address. The guard owns the child, so
/// a panicking scenario still kills and waits it, and a peer that never
/// announces itself panics after the guard exists, not before it.
pub struct ServedPeer {
    child: Child,
    addr: String,
    device_id: String,
    stdout_log: PathBuf,
    stderr_log: PathBuf,
    /// Held so the peer's scratch directory outlives the child process.
    _dir: NodeDir,
    /// Backend-owned resources the caller keeps alive with the peer (a private
    /// database schema), dropped after the child is killed and reaped.
    keepalive: Vec<Box<dyn Any + Send>>,
}

impl ServedPeer {
    /// Start one SQLite-backed peer over `db` with the journal at `journal`,
    /// serving `banks`, and read back the address it bound.
    ///
    /// # Panics
    /// The binary cannot be spawned, or it never reports an address.
    #[must_use]
    pub fn start_sqlite(dir: &NodeDir, db: &Path, journal: &Path, banks: &[&str]) -> Self {
        let peer = Self::start(dir, Some(db), None, journal, banks);
        peer.announce()
    }

    /// Start one Postgres-backed peer over `database_url` with the journal at
    /// `journal`, serving `banks`, and read back the address it bound.
    ///
    /// # Panics
    /// The binary cannot be spawned, or it never reports an address.
    #[must_use]
    pub fn start_postgres(
        dir: &NodeDir,
        database_url: &str,
        journal: &Path,
        banks: &[&str],
    ) -> Self {
        let peer = Self::start(dir, None, Some(database_url), journal, banks);
        peer.announce()
    }

    /// Panic with both logs when the child never reported an address; only
    /// called with the guard in scope, so the failing path still cleans up.
    #[must_use]
    fn announce(self) -> Self {
        assert!(
            !self.addr.is_empty(),
            "the peer never reported a bound address; log:\n{}",
            self.logs()
        );
        self
    }

    fn start(
        dir: &NodeDir,
        db: Option<&Path>,
        database_url: Option<&str>,
        journal: &Path,
        banks: &[&str],
    ) -> Self {
        let credentials = write_credential_store_for(dir, banks);
        let stdout_log = dir.db("serve.stdout.log");
        let stderr_log = dir.db("serve.stderr.log");
        let mut command = clio(dir.path());
        command
            .args(["sync", "serve", "--bind", "127.0.0.1:0"])
            .arg("--token-file")
            .arg(&credentials)
            .args(["--banks", &banks.join(",")])
            .args(["--ttl-secs", SERVE_TTL_SECS, "--output", "json"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(
                std::fs::File::create(&stderr_log).expect("create the peer's stderr log"),
            ));
        match (db, database_url) {
            // The explicit file wins for a SQLite peer.
            (Some(db), _) => {
                command.arg("--db").arg(db);
            }
            // A Postgres peer resolves its backend from the environment URL.
            (None, Some(url)) => {
                command.env("CLIO_DATABASE_URL", url);
            }
            (None, None) => panic!("a peer requires either a database file or a database URL"),
        }
        command.env("CLIO_ERASURE_JOURNAL_FILE", journal);
        // The peer serves with the same key this harness's clients seal under:
        // without it, a sealed row's subject cannot be resolved, so a replay for
        // an erased subject would be stored unread instead of refused.
        command.env("CLIO_SYNC_KEY_FILE", shared_key_file(dir.path()));
        // The peer hashes subject identities under the same deployment salt the
        // driver verifies its proofs with; the driver's own process value is
        // passed through only when the host sets one, so both sides resolve the
        // identical fingerprint.
        if let Ok(salt) = std::env::var("CLIO_COMPLIANCE_SALT") {
            command.env("CLIO_COMPLIANCE_SALT", salt);
        }
        let mut child = command.spawn().expect("spawn clio sync serve");
        let device_id = format!("device-{}", child.id());
        let addr = drain_stdout(&mut child, &stdout_log);
        Self {
            child,
            addr,
            device_id,
            stdout_log,
            stderr_log,
            _dir: NodeDir::hold(dir.path().to_path_buf()),
            keepalive: Vec::new(),
        }
    }

    /// The endpoint a client connects to: a bare `host:port`.
    #[must_use]
    pub fn remote(&self) -> &str {
        &self.addr
    }

    /// The peer deployment's device identity, the value its denial proofs
    /// carry and the driver's delivery target must bind.
    #[must_use]
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Keep one backend-owned resource alive with the peer, dropped only after
    /// the child is killed and reaped.
    pub fn hold(&mut self, value: impl Any + Send + 'static) {
        self.keepalive.push(Box::new(value));
    }

    /// Both captured process logs, stdout first, with a separator between.
    #[must_use]
    pub fn logs(&self) -> String {
        let stdout = std::fs::read_to_string(&self.stdout_log).unwrap_or_default();
        let stderr = std::fs::read_to_string(&self.stderr_log).unwrap_or_default();
        format!("--- stdout ---\n{stdout}--- stderr ---\n{stderr}")
    }

    /// Stop the peer process so its endpoint answers nothing: the offline
    /// window of a reconnect scenario, with the process identity kept, so the
    /// resumed peer is the same deployment.
    ///
    /// # Errors
    /// The stop command cannot be run or reports a failure.
    #[cfg(unix)]
    pub fn stop(&self) -> std::io::Result<()> {
        signal("STOP", self.child.id())
    }

    /// Resume a stopped peer process, restoring the same address and identity.
    ///
    /// # Errors
    /// The resume command cannot be run or reports a failure.
    #[cfg(unix)]
    pub fn resume(&self) -> std::io::Result<()> {
        signal("CONT", self.child.id())
    }

    /// The peer's announce line, read from its stdout log, or the empty string.
    #[must_use]
    pub fn announce_line(&self) -> String {
        std::fs::read_to_string(&self.stdout_log)
            .unwrap_or_default()
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned()
    }
}

impl Drop for ServedPeer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Send one job-control signal to `pid` through the shell's own `kill`, so the
/// harness needs neither an unsafe libc call nor an extra dependency.
#[cfg(unix)]
fn signal(name: &str, pid: u32) -> std::io::Result<()> {
    let status = Command::new("sh")
        .args(["-c", &format!("kill -{name} {pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "kill -{name} {pid} exited with {status}"
        )))
    }
}

/// Drain a child's stdout into its log file on a reader thread, send the first
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
            .expect("open the peer's stdout log");
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
