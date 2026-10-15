// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the Postgres restore's error paths: a target the `--db`
//! URL cannot reach, a dump meta that does not parse, a member the meta
//! declares but the archive does not carry, a key store with nowhere to land,
//! and a load that fails inside the transaction and rolls back.
//!
//! ## Owns
//! - The Postgres restore's refusal and failure assertions
//!
//! ## Does not own
//! - The restore flow (see `backup_restore_flow` and `backup_restore_cli`)
//! - The one-transaction restore (see `clio-store`'s postgres restore)
//!
//! ## Boundary
//!
//! Test-only, and every test serializes on the shared Postgres test lock.
//! Never compiled into production builds.

use super::assemble_tests::{assemble_pg_archive, assemble_pg_archive_with_keys};
use super::tests::{base_url, seed, snapshot};
use crate::backup_cli_fixtures::{
    Scratch, ensure_journal_in, populated_store, run_json_in, run_json_in_with,
};
use clio_backup::dump_meta::DUMP_META_MEMBER;
use clio_store::sqlite_content_digest;
use clio_store::{PG_CONSISTENCY, PG_METHOD, ScopedSchema, lock_pg_test};
use std::path::Path;
use std::process::ExitCode;

#[test]
fn an_unreachable_target_is_refused_before_anything_is_extracted() {
    let _guard = lock_pg_test();
    let a = ScopedSchema::new(&base_url());
    let scratch = Scratch::new("restore-pg-unreachable");
    let kms_a = scratch.join("kms-a.json").display().to_string();
    seed(&a, Path::new(&kms_a), 1);
    let id = snapshot(&scratch, &a, &kms_a);

    let (code, _, err) = run_json_in(
        &scratch,
        &[
            "restore",
            &id,
            "--yes",
            "--db",
            "postgres://clio:hunter2@127.0.0.1:9/nosuchdb",
        ],
    );
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("check `--db`"), "{err}");
    assert!(!err.contains("hunter2"), "the password never echoes: {err}");

    // The refusal ran before the extraction, so only the archive is in the
    // backup directory: no staged member, no pre-restore snapshot.
    let entries: Vec<String> = std::fs::read_dir(scratch.join("backups"))
        .expect("read the backup directory")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries.len(), 1, "{entries:?}");
}

#[test]
fn a_dump_meta_that_does_not_parse_is_refused_with_the_damaged_hint() {
    let _guard = lock_pg_test();
    let base = base_url();
    let a = ScopedSchema::new(&base);
    let b = ScopedSchema::new(&base);
    let scratch = Scratch::new("restore-pg-badmeta");
    seed(&a, &scratch.join("kms-a.json"), 1);
    seed(&b, &scratch.join("kms-b.json"), 0);

    // An archive whose dump meta does not parse: the payload member holds a
    // real digest of invalid bytes, the shape of a half-written archive.
    let backup_dir = scratch.join("backups");
    std::fs::create_dir_all(&backup_dir).expect("create the backup directory");
    let payload = clio_backup::staging::stage_bytes(b"not json at all", &backup_dir, "bad-meta")
        .expect("stage a bad meta");
    let authority = Some(ensure_journal_in(
        backup_dir
            .parent()
            .expect("the backup directory has a parent"),
    ));
    let request = clio_backup::create::CreateRequest {
        backup_dir,
        slug: "clio".to_owned(),
        backend: "postgres".to_owned(),
        database: "clio".to_owned(),
        store_schema: Some("public".to_owned()),
        schema_version: Some("1".to_owned()),
        payload,
        payload_member: DUMP_META_MEMBER.to_owned(),
        additional_members: Vec::new(),
        method: PG_METHOD.to_owned(),
        consistency: PG_CONSISTENCY.to_owned(),
        started_at: "2026-09-27T00:00:00Z".to_owned(),
        duration_ms: 1,
        counts_complete: true,
        keys: None,
        label: None,
        note: None,
        authority,
        tool_version: clio::identity(),
    };
    let report = clio_backup::create_archive(&request).expect("assemble the archive");

    let (code, _, err) = run_json_in(
        &scratch,
        &[
            "restore",
            &report.id,
            "--yes",
            "--allow-missing-keys",
            "--db",
            b.url(),
        ],
    );
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("the archive is damaged"), "{err}");
}

#[test]
fn a_member_the_meta_declares_but_the_archive_does_not_carry_is_refused() {
    let _guard = lock_pg_test();
    let base = base_url();
    let a = ScopedSchema::new(&base);
    let b = ScopedSchema::new(&base);
    let scratch = Scratch::new("restore-pg-phantom");
    let kms_a = scratch.join("kms-a.json").display().to_string();
    seed(&a, Path::new(&kms_a), 1);
    seed(&b, &scratch.join("kms-b.json"), 1);
    let before = clio_store::postgres_target_state(b.url()).expect("measure B before");

    let backup_dir = scratch.join("backups");
    std::fs::create_dir_all(&backup_dir).expect("create the backup directory");
    let mut staged_paths: Vec<std::path::PathBuf> = Vec::new();
    let mut stage = |_table: &clio_store::PgTable,
                     _ordinal: usize|
     -> Result<(std::path::PathBuf, std::fs::File), clio_types::AmError> {
        let (path, file) = clio_backup::staging::begin_staged(&backup_dir, "dump-table")?;
        staged_paths.push(path.clone());
        Ok((path, file))
    };
    let facts = clio_store::snapshot_postgres_into(a.url(), &mut stage).expect("dump A");
    let id = assemble_pg_archive(&backup_dir, &facts, &staged_paths, None, true);

    let (code, _, err) = run_json_in(
        &scratch,
        &[
            "restore",
            &id,
            "--yes",
            "--allow-missing-keys",
            "--db",
            b.url(),
        ],
    );
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("phantom_table"), "{err}");
    assert!(err.contains("the archive is damaged"), "{err}");
    let after = clio_store::postgres_target_state(b.url()).expect("measure B after");
    assert_eq!(after, before, "the refusal left the target as it was");
}

#[test]
fn a_key_store_with_nowhere_to_land_is_reported_as_a_partial_restore() {
    let _guard = lock_pg_test();
    let base = base_url();
    let a = ScopedSchema::new(&base);
    let b = ScopedSchema::new(&base);
    let scratch = Scratch::new("restore-pg-keyswap");
    let kms_a = scratch.join("kms-a.json").display().to_string();
    seed(&a, Path::new(&kms_a), 2);
    seed(&b, &scratch.join("kms-b.json"), 0);
    let id = snapshot(&scratch, &a, &kms_a);

    // The destination's directory does not exist, so the swap cannot place
    // the key store; the load itself still happened.
    let kms_broken = scratch.join("missing-dir").join("kms.json");
    let kms_broken_string = kms_broken.display().to_string();
    let (code, payload, err) = run_json_in_with(
        &scratch,
        &["restore", &id, "--yes"],
        &[
            ("CLIO_DATABASE_URL", b.url()),
            ("CLIO_KMS_FILE", &kms_broken_string),
        ],
    );
    assert_eq!(code, ExitCode::from(3), "{err}");
    assert_eq!(payload["result"], "partial", "{payload}");
    assert_eq!(payload["failed_step"], "key-store swap", "{payload}");
    assert!(payload["failure"].is_string(), "{payload}");
    let state_b = clio_store::postgres_target_state(b.url()).expect("measure B");
    assert_eq!(state_b.count_of("items"), Some(2), "the load happened");
}

#[test]
fn a_count_mismatch_and_a_failed_key_swap_are_both_named() {
    let _guard = lock_pg_test();
    let base = base_url();
    let a = ScopedSchema::new(&base);
    let b = ScopedSchema::new(&base);
    let scratch = Scratch::new("restore-pg-both-fail");
    let kms_a = scratch.join("kms-a.json").display().to_string();
    seed(&a, Path::new(&kms_a), 2);
    seed(&b, &scratch.join("kms-b.json"), 1);

    // The archive's meta claims one more items row than its member can load,
    // so the post-commit count check fails; the key store's destination
    // directory does not exist, so the swap fails too. Two separate problems
    // must both reach the operator.
    let backup_dir = scratch.join("backups");
    std::fs::create_dir_all(&backup_dir).expect("create the backup directory");
    let mut staged_paths: Vec<std::path::PathBuf> = Vec::new();
    let mut stage = |_t: &clio_store::PgTable,
                     _ordinal: usize|
     -> Result<(std::path::PathBuf, std::fs::File), clio_types::AmError> {
        let (path, file) = clio_backup::staging::begin_staged(&backup_dir, "dump-table")?;
        staged_paths.push(path.clone());
        Ok((path, file))
    };
    let mut facts = clio_store::snapshot_postgres_into(a.url(), &mut stage).expect("dump A");
    for table in &mut facts.tables {
        if table.table == "items" {
            table.rows += 1;
        }
    }
    // The archive carries a key member, so the restore attempts the swap. Its
    // destination is under a directory that does not exist, so the swap fails.
    let id = assemble_pg_archive_with_keys(
        &backup_dir,
        &facts,
        &staged_paths,
        None,
        false,
        Some(Path::new(&kms_a)),
    );
    drop(staged_paths);

    let kms_broken = scratch.join("missing-dir").join("kms.json");
    let kms_broken_string = kms_broken.display().to_string();
    let (code, payload, err) = run_json_in_with(
        &scratch,
        &["restore", &id, "--yes"],
        &[
            ("CLIO_DATABASE_URL", b.url()),
            ("CLIO_KMS_FILE", &kms_broken_string),
        ],
    );
    assert_eq!(code, ExitCode::from(3), "{err}");
    assert_eq!(payload["result"], "count-mismatch", "{payload}");
    // Both steps are named: the counts and the key swap. An operator reading
    // only the failed step must not be sent to the wrong problem.
    let failed = payload["failed_step"].as_str().unwrap_or_default();
    assert!(failed.contains("count check"), "{payload}");
    assert!(failed.contains("key-store swap"), "{payload}");
    let failure = payload["failure"].as_str().unwrap_or_default();
    assert!(failure.contains("items"), "the counts failure: {payload}");
    assert!(failure.contains("key store"), "the key failure: {payload}");
    // The load happened; only the count check and the swap failed.
    let state_b = clio_store::postgres_target_state(b.url()).expect("measure B");
    assert_eq!(state_b.count_of("items"), Some(2), "{state_b:?}");
}

#[test]
fn a_load_that_fails_rolls_back_and_leaves_the_target_intact() {
    let _guard = lock_pg_test();
    let base = base_url();
    let a = ScopedSchema::new(&base);
    let b = ScopedSchema::new(&base);
    let scratch = Scratch::new("restore-pg-rollback");
    let kms_a = scratch.join("kms-a.json").display().to_string();
    seed(&a, Path::new(&kms_a), 2);
    seed(&b, &scratch.join("kms-b.json"), 1);
    let before = clio_store::postgres_target_state(b.url()).expect("measure B before");

    // A real dump of A whose items member is rewritten to a row the items
    // table refuses: the load fails inside the transaction and rolls back.
    let backup_dir = scratch.join("backups");
    std::fs::create_dir_all(&backup_dir).expect("create the backup directory");
    let mut staged_paths: Vec<std::path::PathBuf> = Vec::new();
    let mut stage = |_table: &clio_store::PgTable,
                     _ordinal: usize|
     -> Result<(std::path::PathBuf, std::fs::File), clio_types::AmError> {
        let (path, file) = clio_backup::staging::begin_staged(&backup_dir, "dump-table")?;
        staged_paths.push(path.clone());
        Ok((path, file))
    };
    let facts = clio_store::snapshot_postgres_into(a.url(), &mut stage).expect("dump A");
    let id = assemble_pg_archive(
        &backup_dir,
        &facts,
        &staged_paths,
        Some(b"only-one-column\n"),
        false,
    );

    let (code, _, err) = run_json_in(
        &scratch,
        &[
            "restore",
            &id,
            "--yes",
            "--allow-missing-keys",
            "--db",
            b.url(),
        ],
    );
    assert_eq!(code, ExitCode::from(1), "{err}");
    let after = clio_store::postgres_target_state(b.url()).expect("measure B after");
    assert_eq!(after, before, "the failed load left the target as it was");
}

#[test]
fn read_dump_meta_names_the_meta_member_when_its_bytes_do_not_parse() {
    // Built without any database: a well-formed archive whose payload member
    // is the dump meta position but whose bytes are not dump meta.
    let scratch = Scratch::new("restore-pg-meta-direct");
    let backup_dir = scratch.join("backups");
    std::fs::create_dir_all(&backup_dir).expect("create the backup directory");
    let payload = clio_backup::staging::stage_bytes(b"not json at all", &backup_dir, "bad-meta")
        .expect("stage a bad meta");
    let authority = Some(ensure_journal_in(
        backup_dir
            .parent()
            .expect("the backup directory has a parent"),
    ));
    let request = clio_backup::create::CreateRequest {
        backup_dir,
        slug: "clio".to_owned(),
        backend: "postgres".to_owned(),
        database: "clio".to_owned(),
        store_schema: Some("public".to_owned()),
        schema_version: Some("1".to_owned()),
        payload,
        payload_member: DUMP_META_MEMBER.to_owned(),
        additional_members: Vec::new(),
        method: PG_METHOD.to_owned(),
        consistency: PG_CONSISTENCY.to_owned(),
        started_at: "2026-09-27T00:00:00Z".to_owned(),
        duration_ms: 1,
        counts_complete: true,
        keys: None,
        label: None,
        note: None,
        authority,
        tool_version: clio::identity(),
    };
    let report = clio_backup::create_archive(&request).expect("assemble the archive");

    let error = super::read_dump_meta(&report.path).expect_err("the meta bytes do not parse");

    assert_eq!(error.exit_code(), ExitCode::from(2), "{}", error.message());
    assert!(
        error.message().contains(DUMP_META_MEMBER),
        "{}",
        error.message()
    );
    assert!(
        error.hint().unwrap_or_default().contains("damaged"),
        "{:?}",
        error.hint()
    );
}

#[test]
fn a_postgres_archive_is_refused_over_a_sqlite_target_naming_both_backends() {
    let _guard = lock_pg_test();
    let a = ScopedSchema::new(&base_url());
    let scratch = Scratch::new("restore-pg-backend");
    let kms_a = scratch.join("kms-a.json").display().to_string();
    seed(&a, Path::new(&kms_a), 1);
    let store = scratch.join("clio.db");
    populated_store(&store, 3, 1);
    let before = sqlite_content_digest(&store).expect("digest the target");

    let id = snapshot(&scratch, &a, &kms_a);
    let (code, _, err) = run_json_in(&scratch, &["restore", &id, "--yes"]);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("postgres"), "{err}");
    assert!(err.contains("sqlite"), "{err}");
    assert_eq!(
        sqlite_content_digest(&store).expect("digest the target"),
        before,
        "the target is untouched"
    );
}
