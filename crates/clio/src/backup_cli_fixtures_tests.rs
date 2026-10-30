// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the test fixtures the `clio backup` command
//! suites share: the injected run harness, a scratch directory, and a
//! populated SQLite store with a real key store.
//!
//! ## Owns
//! - `run_backup` and `run_backup_with_input`, the injected-stream runners
//!   every backup verb test uses
//! - `Scratch`, a unique temporary directory removed on drop
//! - `populated_store`, a store with items and (optionally) key-store subjects
//! - `archive_with_manifest`, an archive whose manifest fields a test chooses
//!
//! ## Does not own
//! - Any assertion: every fixture is data, never a test of its own
//! - The verb implementations (see `backup_cli` and the command modules)
//!
//! ## Boundary
//!
//! Test-only. Changes to how the shared fixtures are built belong here; adding
//! a case to a verb suite does not.

use crate::cli_error::CliError;
use crate::cli_read::CliIo;
use clio_compliance::copy_digest_of;
use clio_store::{
    DekProvider, DestinationCustodyRow, DestinationCustodyStore, LocalDevKms, SqliteStore, Store,
};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

/// The arguments as owned `String`s.
pub(crate) fn tokens(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

/// An injected env lookup over fixed pairs.
pub(crate) fn env_from<'a>(
    pairs: &'a [(&'a str, &'a str)],
) -> impl Fn(&str) -> Option<String> + 'a {
    move |key: &str| {
        pairs
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| (*value).to_owned())
    }
}

/// Run one `backup` subcommand against injected streams, env, and cwd.
pub(crate) fn run_backup(
    args: &[&str],
    env_pairs: &[(&str, &str)],
    current_dir: Result<PathBuf, CliError>,
    is_tty: bool,
) -> (ExitCode, String, String) {
    run_backup_with_input(args, env_pairs, current_dir, is_tty, &[])
}

/// [`run_backup`] with bytes injected as stdin, so a prompt can be answered
/// without a pseudo-terminal.
pub(crate) fn run_backup_with_input(
    args: &[&str],
    env_pairs: &[(&str, &str)],
    current_dir: Result<PathBuf, CliError>,
    is_tty: bool,
    stdin: &[u8],
) -> (ExitCode, String, String) {
    // Every backup verb writes or opens a key store, and a store slot is sealed
    // under the Clio encryption key. Without the process test key the suite
    // reaches the machine's real keystores, which prompt on macOS and fail
    // outright when both a keychain entry and a key file exist.
    crate::setup_support_tests::ensure_test_key();
    let env = env_from(env_pairs);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut input = std::io::Cursor::new(stdin);
    let code = {
        let mut io = CliIo {
            out: &mut out,
            err: &mut err,
            stdin: &mut input,
            is_tty,
            no_color: true,
            env: &env,
        };
        super::backup_cli::run(&mut io, tokens(args), current_dir)
    };
    (
        code,
        String::from_utf8(out).expect("utf8 out"),
        String::from_utf8(err).expect("utf8 err"),
    )
}

/// Run one `backup` subcommand with JSON as the explicit output mode.
pub(crate) fn run_json(
    args: &[&str],
    env_pairs: &[(&str, &str)],
    current_dir: &Path,
) -> (ExitCode, serde_json::Value, String) {
    let mut with_mode: Vec<&str> = args.to_vec();
    with_mode.push("--output");
    with_mode.push("json");
    let (code, out, err) = run_backup(&with_mode, env_pairs, Ok(current_dir.to_path_buf()), false);
    let payload = serde_json::from_str(out.trim())
        .unwrap_or_else(|_| serde_json::Value::String(out.trim().to_owned()));
    (code, payload, err)
}

/// Run one `backup` subcommand against `scratch`'s store and backup directory.
pub(crate) fn run_in(scratch: &Scratch, args: &[&str], is_tty: bool) -> (ExitCode, String, String) {
    let (env, cwd) = scratch.targets();
    let pairs = env.pairs();
    run_backup(args, &pairs, Ok(cwd), is_tty)
}

/// [`run_in`] with bytes injected as stdin, for the typed-id cases.
pub(crate) fn run_in_with_input(
    scratch: &Scratch,
    args: &[&str],
    is_tty: bool,
    stdin: &[u8],
) -> (ExitCode, String, String) {
    let (env, cwd) = scratch.targets();
    let pairs = env.pairs();
    run_backup_with_input(args, &pairs, Ok(cwd), is_tty, stdin)
}

/// Run one `backup` subcommand with JSON output against `scratch`'s targets.
pub(crate) fn run_json_in(
    scratch: &Scratch,
    args: &[&str],
) -> (ExitCode, serde_json::Value, String) {
    run_json_in_with(scratch, args, &[])
}

/// [`run_json_in`] plus extra environment pairs, for a test that needs to point
/// another resolver somewhere.
pub(crate) fn run_json_in_with(
    scratch: &Scratch,
    args: &[&str],
    extra: &[(&str, &str)],
) -> (ExitCode, serde_json::Value, String) {
    let (env, cwd) = scratch.targets();
    let mut pairs = env.pairs();
    pairs.extend_from_slice(extra);
    run_json(args, &pairs, &cwd)
}

/// A unique scratch directory, removed on drop.
pub(crate) struct Scratch(pub(crate) PathBuf);

impl Scratch {
    pub(crate) fn new(label: &str) -> Self {
        crate::setup_support_tests::ensure_test_key();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is set")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "clio-backup-cli-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create scratch dir");
        Self(path)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// The environment pairs that point every resolver at this scratch store.
    pub(crate) fn env(&self) -> Vec<(String, String)> {
        vec![
            ("CLIO_DATA_DIR".to_owned(), self.0.display().to_string()),
            (
                "CLIO_BACKUP_DIR".to_owned(),
                self.join("backups").display().to_string(),
            ),
        ]
    }

    /// The env pairs and working directory the runner needs.
    ///
    /// The pairs borrow `self`, so a caller keeps the returned `Env` alive for
    /// the length of the call it makes with them.
    pub(crate) fn targets(&self) -> (Env, PathBuf) {
        (Env(self.env()), self.0.clone())
    }
}

/// Owned environment pairs, so a test can hand them to a runner and keep them
/// alive across the call.
pub(crate) struct Env(Vec<(String, String)>);

impl Env {
    /// The pairs as the borrowed slices the runner takes.
    pub(crate) fn pairs(&self) -> Vec<(&str, &str)> {
        self.0
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// One memory item holding `secret`, so a test can look for that plaintext.
pub(crate) fn item(id: &str, secret: &str) -> MemoryItem {
    MemoryItem {
        id: id.to_owned(),
        bank_id: "demo".to_owned(),
        subject_id: format!("subject-{id}"),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.9),
        source_ref: Some("turn_1".to_owned()),
        context: None,
        snapshot: Some(serde_json::json!({ "secret": secret })),
        gist: Some(format!("a note about {secret}")),
        valid_from: "t0".to_owned(),
        valid_until: None,
        tx_from: "t0".to_owned(),
        tx_until: None,
        created_at: "t0".to_owned(),
        updated_at: "t0".to_owned(),
    }
}

/// A file-backed store at `path` holding `items` items, plus a key store with
/// `subjects` subjects beside it.
///
/// The store is opened through its own opener so the schema is the real one,
/// and the key store is written through the real key provider so the file the
/// snapshot carries is the real one.
pub(crate) fn populated_store(path: &Path, items: usize, subjects: usize) {
    crate::setup_support_tests::ensure_test_key();
    let kms_path = key_store_path(path);
    let kms: Arc<dyn DekProvider> =
        Arc::new(LocalDevKms::open_persistent(kms_path).expect("open the persistent key store"));
    let store = SqliteStore::open(path, Arc::clone(&kms)).expect("open the store");
    for index in 0..items {
        let id = format!("item-{index:04}");
        store
            .create_memory_item(&item(&id, "PLANTED-SECRET-abc123"), "harness")
            .expect("write an item");
    }
    for index in 0..subjects {
        kms.ensure_dek(&format!("subject-{index:04}"))
            .expect("write a key-store slot");
    }
}

/// The key-store path beside a store file.
pub(crate) fn key_store_path(store: &Path) -> PathBuf {
    PathBuf::from(format!("{}.kms.json", store.display()))
}

/// Ensure a current erasure journal exists under `scratch` and return its
/// authority, so an archive built here matches the live authority a restore
/// checks against. Idempotent: a current journal is kept as it is.
pub(crate) fn ensure_journal(scratch: &Scratch) -> clio_backup::manifest::ManifestAuthority {
    ensure_journal_in(scratch.path())
}

/// [`ensure_journal`] for a directory the fixture derives rather than a
/// `Scratch`, such as a backup directory's parent.
pub(crate) fn ensure_journal_in(dir: &Path) -> clio_backup::manifest::ManifestAuthority {
    let path = dir.join(clio_config::ERASURE_JOURNAL_FILE_NAME);
    let journal = match clio_compliance::load_journal(&path) {
        clio_compliance::JournalState::Current(journal) => journal,
        _ => clio_compliance::initialize_new_empty(&path).expect("create the erasure journal"),
    };
    clio_backup::manifest::ManifestAuthority {
        deployment_id: journal.deployment_id,
        generation: journal.generation,
        salt_fingerprint: journal.salt_fingerprint,
    }
}

/// Seed the passing copy-custody row the restore route's gate reads for an
/// archive the fixtures built.
///
/// The row is written to the durable store beside the journal in `dir` — the
/// store the route's custody lookup reads — keyed by the snapshot id, holding
/// the digest over the archive's bytes in the delivery write's expression and
/// the receiver the restore route acts as. A fixture archive without such a row
/// is refused as a missing manifest before anything is placed.
pub(crate) fn seed_copy_custody(scratch: &Scratch, id: &str, archive: &Path) {
    seed_copy_custody_in(scratch.path(), id, archive);
}

/// [`seed_copy_custody`] for a directory the fixture derives rather than a
/// `Scratch`, such as a backup directory's parent.
pub(crate) fn seed_copy_custody_in(dir: &Path, id: &str, archive: &Path) {
    let journal = dir.join(clio_config::ERASURE_JOURNAL_FILE_NAME);
    let events = journal.with_file_name(crate::backup_restore_sqlite::RESTORE_EVENTS_FILE);
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open(&events, kms).expect("open the durable store beside the journal");
    let bytes = std::fs::read(archive).expect("read the archive");
    let row = DestinationCustodyRow {
        action_id: id.to_owned(),
        destination_path: archive.display().to_string(),
        custody_owner: "clio".to_owned(),
        required_action: "delivery".to_owned(),
        state: "delivered".to_owned(),
        exception_reason: None,
        copy_digest: Some(copy_digest_of(&bytes)),
        receiver: Some(crate::backup_restore_sqlite::RESTORE_ACTOR.to_owned()),
        updated_at: "2026-01-01T00:00:00Z".to_owned(),
    };
    store
        .save_pending(&row)
        .expect("save the fixture's custody row");
}

/// A well-formed archive whose manifest records a payload digest the payload
/// does not have.
///
/// This is the shape of a tampered or half-copied archive, built through the
/// public API so a verify test does not have to forge tar bytes: the staged
/// payload is real, and only the manifest's record of it is wrong.
pub(crate) fn archive_with_wrong_digest(scratch: &Scratch, store: &Path) -> (String, PathBuf) {
    build_archive(
        scratch,
        store,
        "sqlite",
        Some("1"),
        false,
        Some("0".repeat(64)),
    )
}

/// A populated store at the default path plus one archive of it, returning the
/// snapshot id.
///
/// A fixture: the create is the real command, and a create that did not produce
/// an id fails the fixture loudly instead of letting a test assert nothing.
pub(crate) fn snapshot(scratch: &Scratch, items: usize) -> String {
    populated_store(&scratch.join("clio.db"), items, 1);
    ensure_journal(scratch);
    let (_, payload, _) = run_json_in(scratch, &["create"]);
    let id = payload["snapshot_id"]
        .as_str()
        .expect("the create fixture produced a snapshot id")
        .to_owned();
    let archive = scratch
        .join("backups")
        .join(clio_backup::archive_file_name(&id));
    seed_copy_custody(scratch, &id, &archive);
    id
}

/// An archive whose manifest records the chosen backend, schema version, and
/// key membership, built through the real create path.
///
/// The restore refusals need an archive whose manifest disagrees with the
/// target, and patching archive bytes cannot build one: a byte-level patch
/// inside the gzip stream is corruption, which is a different refusal.
pub(crate) fn archive_with_manifest(
    scratch: &Scratch,
    store: &Path,
    backend: &str,
    schema_version: Option<&str>,
    with_keys: bool,
) -> (String, PathBuf) {
    build_archive(scratch, store, backend, schema_version, with_keys, None)
}

/// The one archive-building path the fixtures share.
fn build_archive(
    scratch: &Scratch,
    store: &Path,
    backend: &str,
    schema_version: Option<&str>,
    with_keys: bool,
    digest: Option<String>,
) -> (String, PathBuf) {
    let backup_dir = scratch.join("backups");
    std::fs::create_dir_all(&backup_dir).expect("create the backup directory");
    let staged = clio_backup::staging::stage_copy(store, &backup_dir, "payload")
        .expect("stage the store as a payload");
    let size = std::fs::metadata(store).expect("stat the store").len();
    let sha256 = digest.unwrap_or_else(|| staged.sha256().to_owned());
    let payload = clio_backup::staging::adopt_staged(staged.path().to_path_buf(), size, sha256);
    let keys = if with_keys {
        clio_store::key_member_source(Some(&key_store_path(store)))
            .expect("read the key store")
            .map(|source| clio_backup::KeySource {
                path: source.path,
                subjects: source.subjects,
            })
    } else {
        None
    };
    let request = clio_backup::create::CreateRequest {
        backup_dir,
        slug: "clio".to_owned(),
        backend: backend.to_owned(),
        database: store
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
        store_schema: None,
        schema_version: schema_version.map(str::to_owned),
        payload,
        payload_member: "store.db".to_owned(),
        additional_members: Vec::new(),
        method: "vacuum-into".to_owned(),
        consistency: "point-in-time".to_owned(),
        started_at: "2026-09-27T00:00:00Z".to_owned(),
        duration_ms: 1,
        counts_complete: true,
        keys,
        label: None,
        note: None,
        authority: Some(ensure_journal(scratch)),
        tool_version: "clio 0.1.0".to_owned(),
    };
    let report = clio_backup::create_archive(&request).expect("create the archive");
    seed_copy_custody(scratch, &report.id, &report.path);
    (report.id, report.path)
}
