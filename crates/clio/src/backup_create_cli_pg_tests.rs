// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests `clio backup create` against a Postgres store: the
//! scoped-schema acceptance path, the key member a key-store file produces,
//! the absence of credentials in every output, and the text report.
//!
//! ## Owns
//! - The Postgres-backed `create` behavior, view, and security assertions
//!
//! ## Does not own
//! - The command implementation (`backup_create_cli`)
//! - The dump transaction and its facts (see `clio-store`'s postgres dump)
//!
//! ## Boundary
//!
//! Test-only, and every test serializes on the shared Postgres test lock.
//! Never compiled into production builds.

use crate::backup_cli_fixtures::{Scratch, run_backup, run_json_in_with};
use clio_store::{DekProvider, LocalDevKms, PostgresStore, ScopedSchema, Store, lock_pg_test};
use serde_json::{Value, json};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

/// The shared test database's URL: the gate runs every test with `CLIO_DATABASE_URL`.
fn base_url() -> String {
    std::env::var("CLIO_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://clio:clio@127.0.0.1:34310/clio".to_owned())
}

/// A private schema whose store was opened once, so its tables exist.
///
/// The store's keys stay in process memory, so a key-store file a test wants
/// is created separately, over a persistent key store.
fn opened_schema(label: &str) -> ScopedSchema {
    let scoped = ScopedSchema::new(&base_url());
    let _store: PostgresStore = scoped.open();
    let _ = label;
    scoped
}

/// A key-store file at `path` holding `subjects` subject slots.
fn key_store(path: &Path, subjects: usize) -> Arc<dyn DekProvider> {
    let kms: Arc<dyn DekProvider> =
        Arc::new(LocalDevKms::open_persistent(path).expect("open the persistent key store"));
    for index in 0..subjects {
        kms.ensure_dek(&format!("subject-{index:04}"))
            .expect("write a key-store slot");
    }
    kms
}

/// The names of the files in `scratch`'s backup directory, sorted.
fn backup_entries(scratch: &Scratch) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(scratch.join("backups"))
        .expect("read the backup directory")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The credential-bearing forms a leak would take.
///
/// The gate's `CLIO_DATABASE_URL` password equals the store name, so the bare
/// password cannot be asserted on; the userinfo pair and the URL scheme are
/// the forms a real leak takes, and neither appears in any honest output.
const CREDENTIAL_NEEDLES: &[&str] = &["clio:clio", "postgres://", "postgresql://"];

/// Every decompressed member of `archive` holds none of the credential forms.
///
/// The members are read back through the crate's own extractor, so the bytes
/// compared are the decompressed member bytes. Grepping the raw archive file
/// would read deflate output, where the literal cannot appear whether or not a
/// credential is present, so that check could never fail.
fn assert_member_bytes_hold_no_credential(archive: &Path) {
    let out = Scratch::new("create-pg-credential-members");
    let manifest = clio_backup::read_manifest(archive).expect("read the manifest");
    let extracted =
        clio_backup::extract_members(archive, &manifest, out.path()).expect("extract the archive");
    let mut members = vec![("payload", &extracted.payload)];
    members.extend(
        extracted
            .dump_tables
            .iter()
            .map(|member| (member.member.as_str(), member)),
    );
    if let Some(key) = extracted.key.as_ref() {
        members.push((key.member.as_str(), key));
    }
    for (name, member) in members {
        let bytes = std::fs::read(member.file.path())
            .unwrap_or_else(|error| panic!("read member {name}: {error}"));
        let text = String::from_utf8_lossy(&bytes);
        for needle in CREDENTIAL_NEEDLES {
            assert!(
                !text.contains(needle),
                "{needle} leaked into member {name} of {archive:?}"
            );
        }
    }
}

#[test]
fn a_scoped_schema_postgres_store_snapshots_with_a_dump_meta_payload() {
    let _guard = lock_pg_test();
    let scoped = opened_schema("create-pg-ok");
    let scratch = Scratch::new("create-pg-ok");

    let (code, payload, err) = run_json_in_with(
        &scratch,
        &["create"],
        &[("CLIO_DATABASE_URL", scoped.url())],
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");

    let state = clio_store::postgres_target_state(scoped.url()).expect("measure the schema after");
    assert_eq!(payload["backend"], "postgres", "{payload}");
    assert_eq!(payload["schema"], state.schema, "{payload}");
    assert_eq!(payload["counts"]["complete"], true, "{payload}");
    assert_eq!(
        payload["counts"]["tables"].as_u64(),
        Some(state.tables.len() as u64),
        "{payload}"
    );
    assert_eq!(payload["counts"]["items"], 0, "{payload}");

    let path = Path::new(payload["path"].as_str().expect("a path"));
    assert!(path.exists(), "{} should exist", path.display());
    let manifest = clio_backup::read_manifest(path).expect("read the manifest");
    assert_eq!(manifest.store.backend, "postgres");
    assert_eq!(
        manifest.store.schema.as_deref(),
        Some(state.schema.as_str())
    );
    assert_eq!(manifest.snapshot.method, "postgres-copy");
    assert_eq!(manifest.payload.member, "dump/meta.json");
    assert!(manifest.counts.complete);

    // Only the archive remains: every staged dump member was adopted into the
    // archive or removed, and the dump meta is a member, not a leftover.
    assert_eq!(
        backup_entries(&scratch).len(),
        1,
        "{:?}",
        backup_entries(&scratch)
    );
}

#[test]
fn a_postgres_snapshot_carries_the_key_store_when_clio_kms_file_names_one() {
    let _guard = lock_pg_test();
    let scoped = opened_schema("create-pg-keys");
    let scratch = Scratch::new("create-pg-keys");
    let kms_path = scratch.join("keys.kms.json");
    let kms_path_string = kms_path.display().to_string();
    key_store(&kms_path, 2);

    let (code, payload, err) = run_json_in_with(
        &scratch,
        &["create"],
        &[
            ("CLIO_DATABASE_URL", scoped.url()),
            ("CLIO_KMS_FILE", &kms_path_string),
        ],
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");

    assert_eq!(payload["keys"]["included"], true, "{payload}");
    assert_eq!(payload["keys"]["subjects"], 2, "{payload}");
    assert_eq!(
        payload["keys"]["warning"], "this archive holds key material; keep it owner-only",
        "{payload}"
    );

    let path = Path::new(payload["path"].as_str().expect("a path"));
    let manifest = clio_backup::read_manifest(path).expect("read the manifest");
    assert!(manifest.keys.included, "{manifest:?}");
    assert_eq!(manifest.keys.subjects, 2);
    assert!(manifest.keys.member.is_some(), "{manifest:?}");
}

#[test]
fn a_snapshot_of_a_url_with_a_password_leaks_no_credential() {
    let _guard = lock_pg_test();
    // The scoped URL is derived from the gate's `CLIO_DATABASE_URL`, which carries
    // a password, so the whole chain — env, target, dump, manifest, archive —
    // runs with one present.
    let scoped = opened_schema("create-pg-credential");
    let scratch = Scratch::new("create-pg-credential");

    let (env, cwd) = scratch.targets();
    let mut pairs = env.pairs();
    pairs.push(("CLIO_DATABASE_URL", scoped.url()));
    let (code, out, err) = run_backup(&["create"], &pairs, Ok(cwd), false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");

    for needle in CREDENTIAL_NEEDLES {
        assert!(!out.contains(needle), "{needle} leaked into stdout: {out}");
        assert!(!err.contains(needle), "{needle} leaked into stderr: {err}");
    }

    // The manifest and the member bytes are read back from disk, so the check
    // covers what a script would read later, not just what was printed.
    let entries = backup_entries(&scratch);
    assert_eq!(entries.len(), 1, "{entries:?}");
    let archive = scratch.join("backups").join(&entries[0]);
    assert_member_bytes_hold_no_credential(&archive);
    let manifest = clio_backup::read_manifest(&archive).expect("read the manifest");
    let manifest_text = serde_json::to_string(&manifest).expect("serialize the manifest");
    for needle in CREDENTIAL_NEEDLES {
        assert!(
            !manifest_text.contains(needle),
            "{needle} leaked: {manifest_text}"
        );
    }
}

/// The sentinel a planted row carries. It appears in the table member, which is
/// where row content belongs, and nowhere else.
const PLANTED: &str = "am-pg-planted-secret-7c41bd";

#[test]
fn a_planted_postgres_row_reaches_only_its_table_member() {
    let _guard = lock_pg_test();
    let scoped = opened_schema("create-pg-planted");
    let scratch = Scratch::new("create-pg-planted");
    {
        let store = scoped.open();
        let mut planted = crate::backup_cli_fixtures::item("am-planted", "am-planted-bank");
        // subject_id is a plaintext column at rest, so the items member
        // carries it while no report, manifest, or meta may.
        planted.subject_id = PLANTED.to_owned();
        store
            .create_memory_item(&planted, "harness")
            .expect("seed the planted row");
    }

    let (env, cwd) = scratch.targets();
    let mut pairs = env.pairs();
    pairs.push(("CLIO_DATABASE_URL", scoped.url()));
    let (code, out, err) = run_backup(&["create"], &pairs, Ok(cwd), false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");

    // 1. stdout.
    assert!(!out.contains(PLANTED), "stdout: {out}");
    // 2. stderr.
    assert!(!err.contains(PLANTED), "stderr: {err}");

    let entries = backup_entries(&scratch);
    assert_eq!(entries.len(), 1, "{entries:?}");
    // 3. the archive file name.
    assert!(!entries[0].contains(PLANTED), "{entries:?}");

    let archive = scratch.join("backups").join(&entries[0]);
    // 4. the manifest, read back from the archive.
    let manifest = clio_backup::read_manifest(&archive).expect("read the manifest");
    let manifest_text = serde_json::to_string(&manifest).expect("serialize the manifest");
    assert!(
        !manifest_text.contains(PLANTED),
        "manifest: {manifest_text}"
    );

    // 5. dump/meta.json, read back out of the archive through its own reader.
    let meta = clio_backup::DumpMeta::read_from_archive(&archive).expect("read the dump meta");
    let meta_text = String::from_utf8_lossy(&meta.to_json_bytes()).into_owned();
    assert!(!meta_text.contains(PLANTED), "dump/meta.json: {meta_text}");

    // 6. the listing.
    let (env, cwd) = scratch.targets();
    let mut pairs = env.pairs();
    pairs.push(("CLIO_DATABASE_URL", scoped.url()));
    let (code, listed, err) = run_backup(&["list"], &pairs, Ok(cwd), true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(!listed.contains(PLANTED), "list: {listed}");

    // The sentinel IS in the archive, in the items member, so the assertions
    // above are not passing because nothing was captured.
    let manifest = clio_backup::read_manifest(&archive).expect("read the manifest");
    assert_eq!(manifest.store.backend, "postgres");
    let state = clio_store::postgres_target_state(scoped.url()).expect("measure the schema");
    assert_eq!(state.count_of("items"), Some(1), "the row was captured");
}

#[test]
fn a_text_create_reports_the_backend_and_the_store_schema() {
    let _guard = lock_pg_test();
    let scoped = opened_schema("create-pg-text");
    let scratch = Scratch::new("create-pg-text");

    let (env, cwd) = scratch.targets();
    let mut pairs = env.pairs();
    pairs.push(("CLIO_DATABASE_URL", scoped.url()));
    let (code, out, err) = run_backup(&["create"], &pairs, Ok(cwd), true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");

    let state = clio_store::postgres_target_state(scoped.url()).expect("measure the schema after");
    assert!(out.contains("backend postgres"), "{out}");
    assert!(out.contains("store schema "), "{out}");
    assert!(out.contains(&state.schema), "{out}");
    assert!(out.contains("counts "), "{out}");
}

#[test]
fn a_postgres_dry_run_measures_the_target_and_names_its_schema_and_tables() {
    let _guard = lock_pg_test();
    let scoped = opened_schema("create-pg-dry-run-live");
    let scratch = Scratch::new("create-pg-dry-run-live");

    let (code, payload, err) = run_json_in_with(
        &scratch,
        &["create", "--dry-run"],
        &[("CLIO_DATABASE_URL", scoped.url())],
    );

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let state = clio_store::postgres_target_state(scoped.url()).expect("measure the schema");
    assert_eq!(payload["schema"], json!(state.schema), "{payload}");
    assert_eq!(payload["tables"], json!(state.table_count()), "{payload}");
    // The size is still unknown: a dry run must not run a dump to learn it.
    assert_eq!(payload["store_bytes"], Value::Null, "{payload}");
    assert_eq!(payload["estimated_bytes_needed"], Value::Null, "{payload}");
    // A dry run writes nothing.
    assert!(!scratch.join("backups").exists());
}

#[test]
fn a_postgres_dry_run_says_the_size_is_unknown_rather_than_reporting_a_zero() {
    let _guard = lock_pg_test();
    let scoped = opened_schema("create-pg-dry-run-text");
    let scratch = Scratch::new("create-pg-dry-run-text");

    let (env, cwd) = scratch.targets();
    let mut pairs = env.pairs();
    pairs.push(("CLIO_DATABASE_URL", scoped.url()));
    let (code, out, err) = run_backup(&["create", "--dry-run"], &pairs, Ok(cwd), true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");

    // The wording must read as unknown. A "0 B" here is a false claim that the
    // snapshot costs nothing.
    assert!(out.contains("store size unknown"), "{out}");
    assert!(out.contains("space needed unknown"), "{out}");
    assert!(!out.contains("estimated space needed 0"), "{out}");
}
