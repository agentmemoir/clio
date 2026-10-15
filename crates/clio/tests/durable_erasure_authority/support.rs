// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared fixtures of the durable erasure
//! authority measurement: per-test scratch directories, journal paths outside
//! every replacement path, file-backed stores, the production denial and
//! erasure calls, and the journal, key-store, and file probes.
//!
//! ## Owns
//! - The synthetic bank, subjects, items, and the deployment salt
//! - `Scratch`, `prepare`, and the unique-name helper
//! - Store opening, the denial and erase fixture calls, and the probes the
//!   scenarios read back
//!
//! ## Does not own
//! - The journal, its checks, and the denial step (see `clio-compliance`)
//! - The scenarios and their assertions (see `membership`, `handoff`,
//!   `sqlite`, `postgres`)
//!
//! ## Boundary
//!
//! Changes to how a fixture is seeded, opened, or observed should normally
//! require changes here. Changes to a scenario's assertions should not.
//!
//! Keep this module focused on fixtures and probes, not scenario logic.

use clio::store::{ComplianceStore, DekProvider, LocalDevKms, PostgresStore, SqliteStore, Store};
use clio::{Actor, Bank, ErrorCode, MemoryItem, OpContext};
use clio_compliance::{
    EraseArgs, EraseOutcome, EraseVecSink, ErasureJournal, JournalState, erase_request,
    load_journal, sha256_hex_bytes,
};
use rusqlite::Connection;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// The Postgres URL used when the environment names none, matching the
/// Makefile and the other loopback-Postgres harnesses.
const COMPOSE_PG: &str = "postgres://clio:clio@127.0.0.1:34310/clio";

/// The one bank every scenario seeds and reads.
pub const BANK: &str = "bank-durable-erasure";

/// The subject the denials and erasures name.
pub const DENIED_SUBJECT: &str = "subject-denied";

/// The subject no denial names.
pub const CONTROL_SUBJECT: &str = "subject-control";

/// The denied subject's only item.
pub const DENIED_ITEM: &str = "item-denied";

/// The deployment salt every scenario hashes subject identities with.
pub fn salt() -> Vec<u8> {
    b"durable-erasure-authority-salt-v1".to_vec()
}

/// The deployment-wide identity of `subject` under [`salt`], as both the
/// journal denial and the store's tombstones record it.
pub fn subject_hash(subject: &str) -> String {
    clio::store::sha256_hex(&salt(), subject)
}

/// The denied subject's deployment-wide identity.
pub fn denied_hash() -> String {
    subject_hash(DENIED_SUBJECT)
}

/// The control subject's deployment-wide identity.
pub fn control_hash() -> String {
    subject_hash(CONTROL_SUBJECT)
}

/// A per-test temp directory, removed on drop.
pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// Create a fresh, unique temp directory for one scenario.
    pub fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(unique(tag));
        std::fs::create_dir_all(&path).expect("create the scratch directory");
        Self { path }
    }

    /// Join a relative path under the scratch directory.
    pub fn join(&self, relative: &str) -> PathBuf {
        self.path.join(relative)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A unique string per call, so two tests never share a path or a name.
pub fn unique(tag: &str) -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("durable-erasure-{tag}-{}-{sequence}", std::process::id())
}

/// A fixture path under the scratch root whose parent directory exists.
///
/// Every journal path built from this is a sibling of the replacement paths —
/// never the database file or its sidecars, the key store, or a backup or
/// staging directory — so a store replacement cannot touch it.
pub fn prepare(scratch: &Scratch, relative: &str) -> PathBuf {
    let path = scratch.join(relative);
    std::fs::create_dir_all(path.parent().expect("the fixture path has a parent"))
        .expect("create the fixture directory");
    path
}

/// The owner-only journal path for one scenario's erasures.
pub fn journal_path(scratch: &Scratch, name: &str) -> PathBuf {
    prepare(scratch, &format!("journal/{name}"))
}

/// The SHA-256 of a file's bytes.
pub fn file_sha256(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    sha256_hex_bytes(&bytes)
}

/// Write `bytes` to `path` as an owner-only file.
pub fn write_owner_only(path: &Path, bytes: &[u8]) {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create the file's directory");
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path).expect("create the owner-only file");
    file.write_all(bytes).expect("write the owner-only file");
}

/// Copy `src` to `dst`, creating `dst`'s parent, and return `dst`.
///
/// The copy keeps the source's permission bits, so a copied journal stays
/// owner-only and loads as a current authority.
pub fn save_copy(src: &Path, dst: &Path) -> PathBuf {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).expect("create the copy directory");
    }
    std::fs::copy(src, dst).expect("copy the fixture file");
    dst.to_path_buf()
}

/// Open a SQLite store backed by the file key store at `kms_path`.
pub fn open_sqlite(db: &Path, kms_path: &Path) -> SqliteStore {
    let kms: Arc<dyn DekProvider> =
        Arc::new(LocalDevKms::open_persistent(kms_path).expect("open the file key store"));
    SqliteStore::open(db, kms).expect("open the sqlite store")
}

/// Open a Postgres store backed by the file key store at `kms_path`.
pub fn open_pg(url: &str, kms_path: &Path) -> PostgresStore {
    let kms: Arc<dyn DekProvider> =
        Arc::new(LocalDevKms::open_persistent(kms_path).expect("open the file key store"));
    PostgresStore::open(url, kms).expect("open the postgres store")
}

/// One synthetic item: dual content, sealed under the subject's DEK on write.
fn item(id: &str, subject: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.to_owned(),
        bank_id: BANK.to_owned(),
        subject_id: subject.to_owned(),
        kind: clio::ItemKind::Semantic,
        category: Some(clio::SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: clio::EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot: Some(serde_json::json!({"entity": "synthetic durable-authority subject"})),
        gist: Some(gist.to_owned()),
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        context: None,
    }
}

/// Write one synthetic item into an open store.
pub fn write_item<S: Store>(store: &S, id: &str, subject: &str, gist: &str) {
    store
        .create_memory_item(&item(id, subject, gist), "harness")
        .expect("write the synthetic item");
}

/// The error code reading one item back produced, or `None` when it decrypted.
pub fn read_error<S: Store + ?Sized>(store: &S, item_id: &str) -> Option<ErrorCode> {
    store
        .get_memory_item(BANK, item_id)
        .err()
        .map(|error| error.code)
}

/// Destroy one subject's DEK through the real compliance erasure call, which
/// records the durable denial in `journal` strictly before the key loss.
pub fn erase_subject(
    store: &dyn ComplianceStore,
    journal: &Path,
    subject: &str,
    request_id: &str,
) -> EraseOutcome {
    let sink = EraseVecSink::default();
    let ctx = OpContext::new(Bank::new(BANK), Actor::Harness);
    let salt = salt();
    erase_request(&EraseArgs {
        store,
        journal_path: journal,
        maint: None,
        ctx: &ctx,
        subject_id: subject,
        legal_basis: "GDPR_ART_17",
        request_id,
        reason_code: None,
        confirm: true,
        salt: &salt,
        timestamp: "2026-09-17T11:00:00Z",
        sink: &sink,
    })
    .expect("run the erasure")
}

/// Load the journal at `path`, requiring the current state.
pub fn load_current(path: &Path) -> ErasureJournal {
    match load_journal(path) {
        JournalState::Current(journal) => journal,
        other => panic!(
            "expected a current journal at {}: {other:?}",
            path.display()
        ),
    }
}

/// Print the journal's identity and bytes hash at `stage`, then return it.
pub fn observe_journal(stage: &str, path: &Path) -> ErasureJournal {
    let journal = load_current(path);
    println!(
        "JOURNAL stage={stage} path={} sha256={} deployment={} generation={} denials={:?}",
        path.display(),
        file_sha256(path),
        journal.deployment_id,
        journal.generation,
        journal.denials,
    );
    journal
}

/// The key-store inventory digest and slot count at `kms_path`.
///
/// The file-backed key store is a SQLite file the store also writes; it is
/// opened read-write with `query_only` so the read can recover a hot journal
/// yet cannot change anything. A store reopen, a journal load, or a read-back
/// probe that minted or resurrected a content key would change the digest, so
/// the scenarios sample it around the store reopen and around the probes.
pub fn keys_inventory_digest(kms_path: &Path) -> (String, u64) {
    let conn = Connection::open(kms_path).expect("open the key store");
    conn.execute_batch("PRAGMA query_only = ON")
        .expect("make the key-store read query-only");
    let mut statement = conn
        .prepare("SELECT subject_id, version, key_b64 IS NOT NULL FROM slots ORDER BY subject_id")
        .expect("prepare the key-store inventory");
    let rows: Vec<String> = statement
        .query_map([], |row| {
            Ok(format!(
                "{}:{}:{}",
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?
            ))
        })
        .expect("query the key-store inventory")
        .collect::<Result<Vec<_>, _>>()
        .expect("read the key-store inventory");
    let count = u64::try_from(rows.len()).expect("the slot count fits in u64");
    (sha256_hex_bytes(rows.join(";").as_bytes()), count)
}

/// The Postgres URL to use: the environment's, or local Compose.
pub fn pg_url() -> String {
    std::env::var("CLIO_DATABASE_URL").unwrap_or_else(|_| COMPOSE_PG.to_owned())
}

/// True when a Postgres server answers at `url`'s host and port within a short
/// budget.
pub fn postgres_reachable(url: &str) -> bool {
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

/// The explicit opt-out from the required Postgres backend.
///
/// A developer with no database sets this to run the rest of the workspace
/// suite; without it an unreachable Postgres fails the measurement instead of
/// passing as a no-op. The name and the truthy values match the Postgres tests
/// in `clio-store`.
const SKIP_PG_TESTS: &str = "CLIO_SKIP_PG_TESTS";

/// True only when the operator explicitly opted out of Postgres-backed tests.
fn postgres_skip_requested() -> bool {
    matches!(
        std::env::var(SKIP_PG_TESTS).as_deref(),
        Ok("1" | "true" | "yes")
    )
}

/// Require a reachable Postgres server for a server-backed scenario.
///
/// Returns the configured URL when a server answers at it. When nothing
/// answers this fails loudly, because the Postgres scenarios are this
/// measurement's only store-backed Postgres evidence and a test that returned
/// green without a server would report a no-op as a result. Only the explicit
/// `CLIO_SKIP_PG_TESTS` opt-out turns the absence into a printed skip and a
/// `None` return, and then the scenario returns without running. The URL is
/// never printed, because it can carry a password into the test log.
pub fn require_postgres() -> Option<String> {
    let url = pg_url();
    if postgres_reachable(&url) {
        return Some(url);
    }
    if postgres_skip_requested() {
        eprintln!(
            "skip durable-erasure-authority postgres: {SKIP_PG_TESTS} is set and no \
             Postgres answers at the configured CLIO_DATABASE_URL"
        );
        return None;
    }
    panic!(
        "the durable-erasure-authority postgres scenarios require a reachable Postgres at \
         CLIO_DATABASE_URL; start the database, or set {SKIP_PG_TESTS}=1 to run the rest \
         of the suite without it"
    );
}
