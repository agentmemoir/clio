// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests `clio backup create`: the refusals, the dry-run plan, the
//! archive it writes, the read-only guarantee, and the absence of plaintext,
//! credentials, and subject identifiers in any output.
//!
//! ## Owns
//! - The `create` behavior, view, and security assertions
//!
//! ## Does not own
//! - The command implementation (`backup_create_cli`)
//! - The archive format (see `clio-backup`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::backup_cli_fixtures::{
    Scratch, populated_store, run_backup, run_in, run_json_in, run_json_in_with,
};
use clio_backup::read_manifest;
use clio_store::sqlite_content_digest;
use serde_json::json;
use std::path::Path;

/// The store a snapshot reads, inside `scratch`.
fn store_path(scratch: &Scratch) -> std::path::PathBuf {
    scratch.join("clio.db")
}

#[test]
fn create_writes_one_archive_and_reports_what_it_holds() {
    let scratch = Scratch::new("create-ok");
    populated_store(&store_path(&scratch), 12, 2);

    let (code, payload, err) = run_json_in(&scratch, &["create"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");

    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert_eq!(payload["backend"], json!("sqlite"), "{payload}");
    assert_eq!(payload["counts"]["items"], json!(12), "{payload}");
    assert_eq!(payload["counts"]["complete"], json!(true), "{payload}");
    assert!(
        payload["counts"]["tables"].as_u64().unwrap_or(0) > 5,
        "the whole schema is counted: {payload}"
    );
    assert_eq!(payload["keys"]["included"], json!(true), "{payload}");
    // One key-store slot per item subject, plus the two the fixture adds.
    assert_eq!(payload["keys"]["subjects"], json!(14), "{payload}");
    // The report says the file holds key material, inside `keys` so the report's
    // top-level shape is unchanged.
    assert_eq!(
        payload["keys"]["warning"],
        json!("this archive holds key material; keep it owner-only"),
        "{payload}"
    );
    assert!(
        payload["payload_bytes"].as_u64().unwrap_or(0) > 0,
        "{payload}"
    );
    assert!(
        payload["archive_bytes"].as_u64().unwrap_or(0) > 0,
        "{payload}"
    );
    assert!(payload["schema_version"].is_string(), "{payload}");

    let id = payload["snapshot_id"].as_str().expect("an id");
    let path = Path::new(payload["path"].as_str().expect("a path"));
    assert!(path.exists(), "{} should exist", path.display());
    let name = path.file_name().expect("a file name").to_string_lossy();
    assert!(name.starts_with(id), "{name} should start with {id}");
    assert!(
        name.ends_with(clio_backup::SNAPSHOT_EXTENSION),
        "{name} should end with the snapshot extension"
    );

    // The manifest is the first member, and the payload digest in it is the
    // digest of the member the archive carries.
    let manifest = read_manifest(path).expect("read the manifest");
    assert_eq!(manifest.payload.member, "store.db");
    assert_eq!(manifest.store.database, "clio.db");
    assert_eq!(manifest.snapshot.method, "vacuum-into");
    assert_eq!(manifest.snapshot.consistency, "point-in-time");
    assert!(manifest.keys.included);
    assert_eq!(manifest.keys.subjects, 14);
    assert_eq!(manifest.keys.member.as_deref(), Some("keys.kms.db"));
    assert!(manifest.counts.complete);
    assert_eq!(
        manifest.payload.sha256.len(),
        64,
        "the manifest carries a real digest: {}",
        manifest.payload.sha256
    );
}

#[test]
fn create_reports_the_snapshot_facts_in_text_mode() {
    let scratch = Scratch::new("create-text");
    populated_store(&store_path(&scratch), 3, 1);

    let (code, out, err) = run_in(&scratch, &["create"], true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("snapshot "), "{out}");
    assert!(out.contains("counts "), "{out}");
    // One slot per item subject plus the one the fixture adds.
    assert!(out.contains("keys included (4 subjects)"), "{out}");
    assert!(out.contains("schema 1"), "{out}");
    // The archive holds key material, and the human report says so.
    assert!(
        out.contains("warning: this archive holds key material; keep it owner-only"),
        "{out}"
    );
}

#[cfg(unix)]
#[test]
fn the_archive_and_its_directory_are_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;
    let scratch = Scratch::new("create-mode");
    populated_store(&store_path(&scratch), 1, 1);

    let (_, payload, err) = run_json_in(&scratch, &["create"]);
    assert_eq!(payload["ok"], json!(true), "{err}");

    let archive = Path::new(payload["path"].as_str().expect("a path"));
    let mode = std::fs::metadata(archive).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "the archive carries key material");
    let dir = scratch.join("backups");
    let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700);
}

#[test]
fn create_leaves_no_staged_file_behind() {
    let scratch = Scratch::new("create-clean");
    populated_store(&store_path(&scratch), 2, 2);

    let (_, payload, err) = run_json_in(&scratch, &["create"]);
    assert_eq!(payload["ok"], json!(true), "{err}");

    let entries: Vec<String> = std::fs::read_dir(scratch.join("backups"))
        .expect("read the backup directory")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries.len(), 1, "only the archive remains: {entries:?}");
    assert!(entries[0].ends_with(".cliobak.tar.gz"), "{entries:?}");
}

#[test]
fn create_does_not_change_the_store() {
    let scratch = Scratch::new("create-readonly");
    let store = store_path(&scratch);
    populated_store(&store, 8, 1);
    let before = sqlite_content_digest(&store).expect("digest the store");
    let size_before = std::fs::metadata(&store).unwrap().len();

    let (_, payload, err) = run_json_in(&scratch, &["create"]);
    assert_eq!(payload["ok"], json!(true), "{err}");

    assert_eq!(
        sqlite_content_digest(&store).expect("digest the store"),
        before,
        "a snapshot is a read-only operation"
    );
    assert_eq!(std::fs::metadata(&store).unwrap().len(), size_before);
    assert!(!scratch.join("clio.db-wal").exists());
}

#[test]
fn the_snapshot_is_logically_equal_to_the_store() {
    let scratch = Scratch::new("create-logical");
    let store = store_path(&scratch);
    populated_store(&store, 30, 0);
    let before = sqlite_content_digest(&store).expect("digest the store");

    let (_, payload, err) = run_json_in(&scratch, &["create"]);
    assert_eq!(payload["ok"], json!(true), "{err}");

    // The payload is a compacted copy, so its bytes differ while its content
    // matches: that is the property the snapshot promises.
    let archive = Path::new(payload["path"].as_str().expect("a path"));
    let extracted = extract_payload(archive);
    assert_eq!(sqlite_content_digest(&extracted).unwrap(), before);
    assert_ne!(
        std::fs::read(&extracted).unwrap(),
        std::fs::read(&store).unwrap(),
        "a VACUUM INTO copy is compacted, never byte-identical"
    );
    let _ = std::fs::remove_file(&extracted);
}

#[test]
fn no_planted_plaintext_reaches_stdout_or_the_listing() {
    let scratch = Scratch::new("create-secret");
    populated_store(&store_path(&scratch), 2, 2);

    let (code, out, err) = run_in(&scratch, &["create"], true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(!out.contains("PLANTED-SECRET-abc123"), "{out}");
    assert!(!err.contains("PLANTED-SECRET-abc123"), "{err}");

    let (code, listed, err) = run_in(&scratch, &["list"], true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(!listed.contains("PLANTED-SECRET-abc123"), "{listed}");
    // Nor does a subject identifier.
    assert!(!listed.contains("subject-0000"), "{listed}");
    assert!(!out.contains("subject-0000"), "{out}");
}

#[test]
fn a_database_url_with_a_password_never_echoes_and_writes_nothing() {
    let scratch = Scratch::new("create-credential");
    // This run cannot connect, so no archive is written and the archive name and
    // manifest are never produced. What it does cover is the failure path: a URL
    // that fails must not echo the password or the URL back to the operator.
    // The name and the manifest of a snapshot that *does* get written are
    // covered by a_snapshot_of_a_url_with_a_password_leaks_no_credential in
    // backup_create_cli_pg_tests, which drives a reachable store.
    let url = "postgres://clio:hunter2@127.0.0.1:9/nosuchdb";

    let (code, out, err) = run_in(&scratch, &["create", "--db", url], false);
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(!err.contains("hunter2"), "the password never echoes: {err}");
    assert!(!err.contains(url), "the URL never echoes: {err}");
    assert!(!out.contains("hunter2"), "the password never prints: {out}");
    assert!(
        std::fs::read_dir(scratch.join("backups")).map_or(0, std::iter::Iterator::count) == 0,
        "nothing was written"
    );
}

#[test]
fn a_memory_store_is_refused() {
    let scratch = Scratch::new("create-memory");

    let (code, _, err) = run_in(&scratch, &["create", "--db", "sqlite::memory:"], false);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("store file"), "{err}");
}

#[test]
fn a_missing_store_is_refused_by_name() {
    let scratch = Scratch::new("create-missing");

    let (code, _, err) = run_in(&scratch, &["create"], false);

    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("no store at"), "{err}");
    assert!(err.contains("clio.db"), "{err}");
}

#[test]
fn a_label_with_a_newline_is_stored_stripped() {
    let scratch = Scratch::new("create-label");
    populated_store(&store_path(&scratch), 1, 0);

    let (code, payload, err) = run_json_in(&scratch, &["create", "--label", "nightly\nrun"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");

    let manifest = read_manifest(Path::new(payload["path"].as_str().expect("a path")))
        .expect("read the manifest");
    assert_eq!(manifest.label.as_deref(), Some("nightlyrun"));
}

#[test]
fn a_dry_run_writes_nothing_and_prints_the_plan() {
    let scratch = Scratch::new("create-dry");
    populated_store(&store_path(&scratch), 5, 1);

    let (code, payload, err) = run_json_in(&scratch, &["create", "--dry-run"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    assert_eq!(
        payload["store"],
        json!(store_path(&scratch).display().to_string())
    );
    assert_eq!(
        payload["path"],
        json!(scratch.join("backups").display().to_string())
    );
    assert_eq!(payload["existing_snapshots"], json!(0), "{payload}");
    assert!(
        payload["estimated_bytes_needed"].as_u64().unwrap_or(0) > 0,
        "{payload}"
    );
    let archives: Vec<String> = std::fs::read_dir(scratch.join("backups"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        archives.is_empty(),
        "a dry run writes nothing: {archives:?}"
    );
}

#[test]
fn two_creates_in_the_same_second_produce_two_archives() {
    let scratch = Scratch::new("create-twice");
    populated_store(&store_path(&scratch), 2, 0);

    let (_, first, err) = run_json_in(&scratch, &["create"]);
    assert_eq!(first["ok"], json!(true), "{err}");
    let (_, second, err) = run_json_in(&scratch, &["create"]);
    assert_eq!(second["ok"], json!(true), "{err}");

    assert_ne!(first["snapshot_id"], second["snapshot_id"]);
    assert_ne!(first["path"], second["path"]);
}

#[test]
fn create_help_and_bad_flags_fail_closed() {
    let scratch = Scratch::new("create-help");

    let (code, out, _) = run_in(&scratch, &["create", "--help"], false);
    assert_eq!(code, ExitCode::SUCCESS);
    assert!(out.contains("usage: clio backup create"), "{out}");
    assert!(out.contains("--dry-run"), "{out}");

    let (code, _, err) = run_in(&scratch, &["create", "--frobnicate"], false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    let (code, _, err) = run_in(&scratch, &["create", "extra"], false);
    assert_eq!(code, ExitCode::from(2), "{err}");
}

/// Extract the payload member of an archive into a temp file beside it.
fn extract_payload(archive: &Path) -> std::path::PathBuf {
    use clio_backup::container::ArchiveReader;
    let file = std::fs::File::open(archive).expect("open the archive");
    let mut reader = ArchiveReader::new(file).expect("read the archive");
    assert!(reader.next_member().expect("manifest").is_some());
    reader
        .read_current(&mut std::io::sink())
        .expect("skip the manifest");
    let header = reader.next_member().expect("payload").expect("a member");
    let out = archive.with_extension("extracted.db");
    let mut sink = std::fs::File::create(&out).expect("create the extracted file");
    reader.read_current(&mut sink).expect("payload bytes");
    drop(sink);
    assert_eq!(header.name, "store.db");
    out
}

#[test]
fn create_with_a_failed_current_directory_is_an_internal_error() {
    let error = CliError::operational("internal", "cannot determine the current directory: boom");

    let (code, _, err) = run_backup(&["create"], &[], Err(error), false);

    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("current directory"), "{err}");
}

#[test]
fn create_refuses_an_output_mode_it_does_not_know() {
    let scratch = Scratch::new("create-bad-output");
    let cwd = Ok(scratch.path().to_path_buf());

    let (code, _, err) = run_backup(&["create", "--output", "nonsense"], &[], cwd, false);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("--output must be"), "{err}");
    assert!(err.contains("nonsense"), "{err}");
}

#[test]
fn a_postgres_dry_run_reports_an_unknown_size_not_a_zero_and_writes_nothing() {
    let scratch = Scratch::new("create-pg-dry-run");
    let url = "postgres://clio:clio@127.0.0.1:9/nosuchdb";

    let (code, payload, err) = run_json_in_with(
        &scratch,
        &["create", "--db", url, "--dry-run"],
        &[("CLIO_DATABASE_URL", url)],
    );

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    assert_eq!(payload["backend"], json!("postgres"), "{payload}");
    // A Postgres store is not a file, so the plan reports the size as unknown.
    // A zero would read as "this costs nothing", which is a false claim.
    assert_eq!(payload["store_bytes"], Value::Null, "{payload}");
    assert_eq!(payload["estimated_bytes_needed"], Value::Null, "{payload}");
    // The target could not be reached, so the schema and the table count are
    // unknown too rather than reported as an empty store.
    assert_eq!(payload["schema"], Value::Null, "{payload}");
    assert_eq!(payload["tables"], Value::Null, "{payload}");
    // The report shows the credential-free database name, never the URL's
    // password.
    assert_eq!(payload["database"], json!("nosuchdb"), "{payload}");
    assert_eq!(payload["store"], json!("nosuchdb"), "{payload}");
    assert!(
        !payload.to_string().contains("clio:clio"),
        "the dry-run report carries a credential: {payload}"
    );
    // A dry run writes nothing: not even the backup directory.
    assert!(!scratch.join("backups").exists());
}
