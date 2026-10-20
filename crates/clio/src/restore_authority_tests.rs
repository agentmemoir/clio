// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of `restore_authority`: an archive
//! with no recorded authority, a SQLite payload whose tombstone hash or subject
//! key becomes a reference, a key store whose slot id becomes a reference, an
//! undeclared member that stays unknown, a Postgres dump whose subject tables
//! become references, and a clean archive that yields no reference.
//!
//! ## Owns
//! - The SQLite and Postgres archive fixtures these cases build
//! - Every assertion about `classify_archive`
//!
//! ## Does not own
//! - The authority check those facts feed (`clio-compliance` owns it)
//! - The extraction or manifest (`clio-backup` owns those)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{ClassifyRefusal, classify_archive};
use clio_backup::manifest::ManifestAuthority;
use clio_compliance::MemberFacts;
use clio_types::ErrorCode;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Calls served so far, so two fixtures never share a directory.
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A scratch directory removed when the guard drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
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

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An archive and the manifest read back from it.
struct Built {
    path: PathBuf,
    manifest: clio_backup::BackupManifest,
}

/// The authority the fixtures record.
fn authority() -> ManifestAuthority {
    ManifestAuthority {
        deployment_id: "deploy-1".to_owned(),
        generation: 3,
        salt_fingerprint: Some(clio_compliance::tombstone_salt_fingerprint()),
    }
}

/// Create a SQLite database at `path` holding the tables `statements` build.
fn sqlite_file(path: &Path, statements: &[&str]) {
    let connection = rusqlite::Connection::open(path).expect("create the fixture database");
    for statement in statements {
        connection
            .execute_batch(statement)
            .expect("run a fixture statement");
    }
}

/// Assemble a SQLite archive over `payload`, optionally carrying `key`.
fn build_sqlite_archive(
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
/// names in dump order, and its `COPY` text.
fn build_postgres_archive(scratch: &Scratch, tables: &[(&str, &[&str], &[u8])]) -> Built {
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
        keys: None,
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
fn clean_payload(scratch: &Scratch) -> PathBuf {
    let path = scratch.join("store.db");
    sqlite_file(&path, &["CREATE TABLE items (id TEXT PRIMARY KEY)"]);
    path
}

#[test]
fn an_archive_without_authority_is_refused() {
    let scratch = Scratch::new("no-authority");
    let payload = clean_payload(&scratch);
    let built = build_sqlite_archive(&scratch, &payload, None, None, None);

    let refusal = classify_archive(&built.path, &built.manifest).expect_err("no authority refuses");

    assert!(matches!(refusal, ClassifyRefusal::ArchiveMissingAuthority));
    assert_eq!(refusal.code(), ErrorCode::NotFound);
}

#[test]
fn a_tombstone_hash_becomes_a_reference() {
    let scratch = Scratch::new("tombstone");
    let hash = "a".repeat(64);
    let payload = scratch.join("store.db");
    sqlite_file(
        &payload,
        &[
            "CREATE TABLE compliance_tombstones (subject_hash TEXT NOT NULL)",
            &format!("INSERT INTO compliance_tombstones (subject_hash) VALUES ('{hash}')"),
        ],
    );
    let built = build_sqlite_archive(&scratch, &payload, None, Some(authority()), None);

    let classified = classify_archive(&built.path, &built.manifest).expect("classify");

    assert!(
        classified
            .members
            .contains(&MemberFacts::Reference { subject_hash: hash }),
        "{:?}",
        classified.members
    );
    assert_eq!(classified.claim.deployment_id, "deploy-1");
    assert_eq!(classified.claim.generation, 3);
    assert_eq!(
        classified.claim.salt_fingerprint,
        Some(clio_compliance::tombstone_salt_fingerprint()),
        "the manifest authority's salt fingerprint reaches the claim"
    );
}

/// An archive whose recorded salt fingerprint differs from the current
/// deployment salt is refused before any member is read.
#[test]
fn an_archive_with_a_foreign_salt_fingerprint_is_refused() {
    let scratch = Scratch::new("salt-mismatch");
    let payload = clean_payload(&scratch);
    let mut authority = authority();
    authority.salt_fingerprint = Some("a-different-salt".to_owned());
    let built = build_sqlite_archive(&scratch, &payload, None, Some(authority), None);

    let refusal =
        classify_archive(&built.path, &built.manifest).expect_err("a salt mismatch refuses");

    assert!(matches!(refusal, ClassifyRefusal::SaltFingerprintMismatch));
    assert_eq!(refusal.code(), ErrorCode::Forbidden);
}

/// An archive whose authority records no salt fingerprint fails closed.
#[test]
fn an_archive_with_an_absent_salt_fingerprint_is_refused() {
    let scratch = Scratch::new("salt-absent");
    let payload = clean_payload(&scratch);
    let mut authority = authority();
    authority.salt_fingerprint = None;
    let built = build_sqlite_archive(&scratch, &payload, None, Some(authority), None);

    let refusal =
        classify_archive(&built.path, &built.manifest).expect_err("an absent fingerprint refuses");

    assert!(matches!(refusal, ClassifyRefusal::SaltFingerprintMismatch));
}

#[test]
fn a_payload_subject_key_id_is_hashed_to_a_reference() {
    let scratch = Scratch::new("subject-key");
    let id = "subject-0007";
    let payload = scratch.join("store.db");
    sqlite_file(
        &payload,
        &[
            "CREATE TABLE subject_keys (subject_id TEXT PRIMARY KEY, dek_ref TEXT)",
            &format!("INSERT INTO subject_keys (subject_id, dek_ref) VALUES ('{id}', 'dek')"),
        ],
    );
    let built = build_sqlite_archive(&scratch, &payload, None, Some(authority()), None);
    let expected = clio_store::sha256_hex(&clio_compliance::tombstone_salt(), id);

    let classified = classify_archive(&built.path, &built.manifest).expect("classify");

    assert!(
        classified.members.contains(&MemberFacts::Reference {
            subject_hash: expected,
        }),
        "{:?}",
        classified.members
    );
}

#[test]
fn a_key_store_slot_id_is_hashed_to_a_reference() {
    let scratch = Scratch::new("key-slot");
    let id = "subject-0042";
    let key = scratch.join("keys.kms.db");
    sqlite_file(
        &key,
        &[
            "CREATE TABLE slots (subject_id TEXT PRIMARY KEY, version INTEGER NOT NULL, key_b64 TEXT)",
            &format!("INSERT INTO slots (subject_id, version, key_b64) VALUES ('{id}', 1, 'AAAA')"),
        ],
    );
    let payload = clean_payload(&scratch);
    let built = build_sqlite_archive(&scratch, &payload, Some(&key), Some(authority()), None);
    let expected = clio_store::sha256_hex(&clio_compliance::tombstone_salt(), id);

    let classified = classify_archive(&built.path, &built.manifest).expect("classify");

    assert!(
        classified.members.contains(&MemberFacts::Reference {
            subject_hash: expected,
        }),
        "{:?}",
        classified.members
    );
}

#[test]
fn an_undeclared_member_stays_unknown() {
    let scratch = Scratch::new("extra-member");
    let payload = clean_payload(&scratch);
    let built = build_sqlite_archive(
        &scratch,
        &payload,
        None,
        Some(authority()),
        Some(("extra.txt", b"not a declared member")),
    );

    let classified = classify_archive(&built.path, &built.manifest).expect("classify");

    assert!(
        classified.members.contains(&MemberFacts::Unknown),
        "{:?}",
        classified.members
    );
}

#[test]
fn a_clean_archive_has_no_reference() {
    let scratch = Scratch::new("clean");
    let payload = clean_payload(&scratch);
    let built = build_sqlite_archive(&scratch, &payload, None, Some(authority()), None);

    let classified = classify_archive(&built.path, &built.manifest).expect("classify");

    assert!(
        classified
            .members
            .iter()
            .all(|member| matches!(member, MemberFacts::None)),
        "{:?}",
        classified.members
    );
}

#[test]
fn a_postgres_subject_table_becomes_a_reference_and_content_does_not() {
    let scratch = Scratch::new("postgres");
    let hash = "b".repeat(64);
    let tombstone_copy = format!("{hash}\tCOMPLIANCE_CRYPTO_SHRED\n");
    let subject_id = "subject-0009";
    let subject_copy = format!("{subject_id}\tdek\tactive\n");
    let built = build_postgres_archive(
        &scratch,
        &[
            (
                "compliance_tombstones",
                &["subject_hash", "reason_code"],
                tombstone_copy.as_bytes(),
            ),
            ("items", &["id", "subject_id"], b"item-1\tsubject-0009\n"),
            (
                "subject_keys",
                &["subject_id", "dek_ref", "status"],
                subject_copy.as_bytes(),
            ),
            ("triples", &["id"], b"triple-1\n"),
        ],
    );
    let expected = clio_store::sha256_hex(&clio_compliance::tombstone_salt(), subject_id);

    let classified = classify_archive(&built.path, &built.manifest).expect("classify");

    assert!(
        classified
            .members
            .contains(&MemberFacts::Reference { subject_hash: hash }),
        "{:?}",
        classified.members
    );
    assert!(
        classified.members.contains(&MemberFacts::Reference {
            subject_hash: expected,
        }),
        "{:?}",
        classified.members
    );
    // The subject key and the item row name the same subject, so one hash
    // covers both, and the content table names no subject.
    assert!(
        classified
            .members
            .iter()
            .filter(|member| matches!(member, MemberFacts::None))
            .count()
            >= 3,
        "{:?}",
        classified.members
    );
}
