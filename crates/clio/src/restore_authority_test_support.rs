// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the archive fixtures the `restore_authority`
//! test modules share: a private scratch directory, a freshly built SQLite
//! archive, a freshly built Postgres archive, and the payload and key-store
//! shapes the cases need.
//!
//! ## Owns
//! - `Scratch`, `Built`, `authority`, and the fixture builders
//!
//! ## Does not own
//! - Any test (see `restore_authority*_tests`)
//! - The classification itself (`restore_authority`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on building one archive to classify.

use clio_backup::manifest::ManifestAuthority;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far, so two fixtures never share a directory.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A scratch directory removed when the guard drops.
pub(super) struct Scratch(PathBuf);

impl Scratch {
    pub(super) fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is set")
            .as_nanos();
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "clio-restore-authority-test-{label}-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create the scratch directory");
        Self(path)
    }

    pub(super) fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An archive and the manifest read back from it.
pub(super) struct Built {
    pub(super) path: PathBuf,
    pub(super) manifest: clio_backup::BackupManifest,
}

/// The authority the fixtures record.
pub(super) fn authority() -> ManifestAuthority {
    ManifestAuthority {
        deployment_id: "deploy-1".to_owned(),
        generation: 3,
        salt_fingerprint: Some(clio_compliance::tombstone_salt_fingerprint()),
    }
}

/// Create a SQLite database at `path` holding the tables `statements` build.
pub(super) fn sqlite_file(path: &Path, statements: &[&str]) {
    let connection = rusqlite::Connection::open(path).expect("create the fixture database");
    for statement in statements {
        connection
            .execute_batch(statement)
            .expect("run a fixture statement");
    }
}

/// Assemble a SQLite archive over `payload`, optionally carrying `key`.
pub(super) fn build_sqlite_archive(
    scratch: &Scratch,
    payload: &Path,
    key: Option<&Path>,
    authority: Option<ManifestAuthority>,
    extra: Option<(&str, &[u8])>,
) -> Built {
    let backup_dir = scratch.join("backups");
    std::fs::create_dir_all(&backup_dir).expect("create the backup directory");
    let staged = clio_backup::staging::stage_copy(payload, &backup_dir, "payload")
        .expect("stage the payload");
    let additional_members = match extra {
        Some((name, bytes)) => vec![clio_backup::create::StagedMember {
            name: name.to_owned(),
            file: clio_backup::staging::stage_bytes(bytes, &backup_dir, "extra")
                .expect("stage the extra member"),
        }],
        None => Vec::new(),
    };
    let request = clio_backup::create::CreateRequest {
        backup_dir,
        slug: "clio".to_owned(),
        backend: "sqlite".to_owned(),
        database: "clio".to_owned(),
        store_schema: None,
        schema_version: Some("1".to_owned()),
        payload: staged,
        payload_member: "store.db".to_owned(),
        additional_members,
        method: "vacuum-into".to_owned(),
        consistency: "point-in-time".to_owned(),
        started_at: "2026-09-27T00:00:00Z".to_owned(),
        duration_ms: 1,
        counts_complete: true,
        keys: key.map(|path| clio_backup::create::KeySource {
            path: path.to_path_buf(),
            subjects: 1,
        }),
        label: None,
        note: None,
        authority,
        tool_version: "clio 0.1.0".to_owned(),
    };
    let report = clio_backup::create_archive(&request).expect("create the archive");
    let manifest = clio_backup::read_manifest(&report.path).expect("read the manifest");
    Built {
        path: report.path,
        manifest,
    }
}

/// Assemble a Postgres archive over `tables`: each is a table name, its column
/// names in dump order, and its `COPY` text. `key` adds a key member.
pub(super) fn build_postgres_archive(
    scratch: &Scratch,
    tables: &[(&str, &[&str], &[u8])],
    key: Option<&Path>,
) -> Built {
    use clio_backup::dump_meta::{
        DUMP_FORMAT_VERSION, DUMP_KIND, DUMP_META_MEMBER, DumpColumn, DumpMeta, DumpTable,
    };
    let backup_dir = scratch.join("backups");
    std::fs::create_dir_all(&backup_dir).expect("create the backup directory");
    let mut dump_tables = Vec::new();
    let mut members = Vec::new();
    for (index, (table, columns, bytes)) in tables.iter().enumerate() {
        let member = format!("dump/tables/{:03}_{table}.txt", index + 1);
        let staged = clio_backup::staging::stage_bytes(bytes, &backup_dir, "dump-table")
            .expect("stage a dump table");
        dump_tables.push(DumpTable {
            table: (*table).to_owned(),
            member: member.clone(),
            bytes: staged.bytes(),
            sha256: staged.sha256().to_owned(),
            rows: 1,
            columns: columns
                .iter()
                .enumerate()
                .map(|(ordinal, name)| DumpColumn {
                    name: (*name).to_owned(),
                    ordinal: i32::try_from(ordinal).expect("a small column count") + 1,
                    type_name: "text".to_owned(),
                })
                .collect(),
        });
        members.push(clio_backup::create::StagedMember {
            name: member,
            file: staged,
        });
    }
    let meta = DumpMeta {
        kind: DUMP_KIND.to_owned(),
        format_version: DUMP_FORMAT_VERSION,
        backend: "postgres".to_owned(),
        schema: "public".to_owned(),
        database: "clio".to_owned(),
        extensions: Vec::new(),
        banks: Vec::new(),
        method: "postgres-copy".to_owned(),
        consistency: "point-in-time".to_owned(),
        tables: dump_tables,
    };
    let payload =
        clio_backup::staging::stage_bytes(&meta.to_json_bytes(), &backup_dir, "dump-meta")
            .expect("stage the dump meta");
    let request = clio_backup::create::CreateRequest {
        backup_dir,
        slug: "clio".to_owned(),
        backend: "postgres".to_owned(),
        database: "clio".to_owned(),
        store_schema: Some("public".to_owned()),
        schema_version: Some("1".to_owned()),
        payload,
        payload_member: DUMP_META_MEMBER.to_owned(),
        additional_members: members,
        method: "postgres-copy".to_owned(),
        consistency: "point-in-time".to_owned(),
        started_at: "2026-09-27T00:00:00Z".to_owned(),
        duration_ms: 1,
        counts_complete: true,
        keys: key.map(|path| clio_backup::create::KeySource {
            path: path.to_path_buf(),
            subjects: 1,
        }),
        label: None,
        note: None,
        authority: Some(authority()),
        tool_version: "clio 0.1.0".to_owned(),
    };
    let report = clio_backup::create_archive(&request).expect("create the archive");
    let manifest = clio_backup::read_manifest(&report.path).expect("read the manifest");
    Built {
        path: report.path,
        manifest,
    }
}

/// A clean SQLite payload: a store with no subject table at all.
pub(super) fn clean_payload(scratch: &Scratch) -> PathBuf {
    let path = scratch.join("store.db");
    sqlite_file(&path, &["CREATE TABLE items (id TEXT PRIMARY KEY)"]);
    path
}

/// An empty key store: the `slots` table and no key in it.
pub(super) fn empty_key_store(scratch: &Scratch) -> PathBuf {
    let path = scratch.join("keys.kms.db");
    sqlite_file(
        &path,
        &[
            "CREATE TABLE slots (subject_id TEXT PRIMARY KEY, version INTEGER NOT NULL, key_b64 TEXT)",
        ],
    );
    path
}

/// A key store holding one slot for `id` with the given `key_b64` value.
pub(super) fn key_store_with_slot(scratch: &Scratch, name: &str, id: &str, key: &str) -> PathBuf {
    let path = scratch.join(name);
    sqlite_file(
        &path,
        &[
            "CREATE TABLE slots (subject_id TEXT PRIMARY KEY, version INTEGER NOT NULL, key_b64 TEXT)",
            &format!("INSERT INTO slots (subject_id, version, key_b64) VALUES ('{id}', 1, {key})"),
        ],
    );
    path
}
