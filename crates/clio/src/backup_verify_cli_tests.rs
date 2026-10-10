// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests `clio backup verify`: a good archive, a flipped payload
//! byte, a truncated archive, an unknown id, `--latest`, the store-down case,
//! the hand-built unknown-backend refusal, and the flag refusals.
//!
//! ## Owns
//! - The `verify` behavior, view, exit-code, and refusal assertions
//! - The hand-built unknown-backend archive the manifest refusal turns on
//!
//! ## Does not own
//! - The command implementation (`backup_verify_cli`)
//! - Archive creation (see `backup_create_cli_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::backup_cli_fixtures::{
    Scratch, populated_store, run_backup, run_in, run_json_in, run_json_in_with,
};
use clio_backup::staging;
use serde_json::json;
use std::path::PathBuf;

/// Take one snapshot and return its id and archive path.
fn snapshot(scratch: &Scratch) -> (String, PathBuf) {
    populated_store(&scratch.join("clio.db"), 4, 1);
    let (code, payload, err) = run_json_in(scratch, &["create"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    (
        payload["snapshot_id"].as_str().expect("an id").to_owned(),
        PathBuf::from(payload["path"].as_str().expect("a path")),
    )
}

#[test]
fn a_good_archive_passes_every_member_and_exits_zero() {
    let scratch = Scratch::new("verify-ok");
    let (id, _) = snapshot(&scratch);

    let (code, payload, err) = run_json_in(&scratch, &["verify", &id]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert_eq!(payload["snapshot_id"], json!(id), "{payload}");
    assert_eq!(
        payload["failed"].as_array().map(Vec::len),
        Some(0),
        "{payload}"
    );
    let members = payload["members"].as_array().expect("members");
    assert!(
        members
            .iter()
            .any(|check| check["member"] == json!("store.db"))
    );
    assert!(
        members
            .iter()
            .any(|check| check["member"] == json!("store.db integrity")
                && check["ok"] == json!(true)),
        "the store's own integrity check ran and passed: {payload}"
    );
    assert!(
        members
            .iter()
            .any(|check| check["member"] == json!("keys.kms.db"))
    );
}

#[test]
fn the_text_view_names_each_member_check() {
    let scratch = Scratch::new("verify-text");
    let (id, _) = snapshot(&scratch);

    let (code, out, err) = run_in(&scratch, &["verify", &id], true);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains(&id), "{out}");
    assert!(out.contains("pass"), "{out}");
    assert!(out.contains("store.db"), "{out}");
    assert!(out.contains("integrity"), "{out}");
}

#[test]
fn a_payload_whose_digest_disagrees_fails_and_names_the_member() {
    let scratch = Scratch::new("verify-digest");
    let store = scratch.join("clio.db");
    populated_store(&store, 4, 1);
    // A well-formed archive whose manifest records a digest the payload does
    // not have: what a tampered or half-copied archive looks like to a verify
    // pass, and the digest is what catches it.
    let (id, _path) = crate::backup_cli_fixtures::archive_with_wrong_digest(&scratch, &store);

    let (code, payload, err) = run_json_in(&scratch, &["verify", &id]);

    assert_eq!(code, ExitCode::from(1), "{err}");
    assert_eq!(payload["ok"], json!(false), "{payload}");
    let failed = payload["failed"].as_array().expect("an array");
    assert!(failed.iter().any(|name| name == "store.db"), "{payload}");
}

#[test]
fn a_truncated_archive_is_refused_with_a_specific_message() {
    let scratch = Scratch::new("verify-truncated");
    let (id, path) = snapshot(&scratch);
    let bytes = std::fs::read(&path).expect("read the archive");
    std::fs::write(&path, &bytes[..bytes.len() / 2]).expect("truncate");

    let (code, _, err) = run_json_in(&scratch, &["verify", &id]);

    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(
        err.contains("truncated") || err.contains("corrupt"),
        "{err}"
    );
}

#[test]
fn latest_resolves_the_newest_snapshot() {
    let scratch = Scratch::new("verify-latest");
    populated_store(&scratch.join("clio.db"), 2, 1);
    for _ in 0..2 {
        let (code, _, err) = run_json_in(&scratch, &["create"]);
        assert_eq!(code, ExitCode::SUCCESS, "{err}");
    }
    let (code, listing, err) = run_json_in(&scratch, &["list"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let newest = listing["snapshots"][0]["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let (code, payload, err) = run_json_in(&scratch, &["verify", "--latest"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["snapshot_id"], json!(newest), "{payload}");
}

#[test]
fn latest_in_an_empty_directory_is_not_found() {
    let scratch = Scratch::new("verify-latest-empty");
    std::fs::create_dir_all(scratch.join("backups")).expect("create the directory");

    let (code, _, err) = run_json_in(&scratch, &["verify", "--latest"]);

    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("no snapshot to verify"), "{err}");
    let error: Value = serde_json::from_str(err.trim()).expect("error envelope");
    assert_eq!(error["ok"], json!(false), "{error}");
    assert_eq!(error["code"], json!("not_found"), "{error}");
}

#[test]
fn an_unknown_id_is_not_found_and_suggests_the_closest_one() {
    let scratch = Scratch::new("verify-unknown");
    let (id, _) = snapshot(&scratch);
    let mut wrong = id.clone();
    wrong.push('x');

    let (code, _, err) = run_json_in(&scratch, &["verify", &wrong]);

    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains(&format!("did you mean `{id}`")), "{err}");
    let error: Value = serde_json::from_str(err.trim()).expect("error envelope");
    assert_eq!(error["code"], json!("not_found"), "{error}");
}

#[test]
fn verify_needs_exactly_one_selector() {
    let scratch = Scratch::new("verify-selector");
    let (id, _) = snapshot(&scratch);

    let (code, _, err) = run_json_in(&scratch, &["verify"]);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("needs a snapshot id"), "{err}");

    let (code, _, err) = run_json_in(&scratch, &["verify", &id, "--latest"]);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("not both"), "{err}");

    let (code, _, err) = run_json_in(&scratch, &["verify", &id, &id]);
    assert_eq!(code, ExitCode::from(2), "{err}");
}

#[test]
fn verify_refuses_the_store_flags_and_a_foreign_backend() {
    let scratch = Scratch::new("verify-refuse");
    let (id, _) = snapshot(&scratch);

    let (code, _, err) = run_json_in(&scratch, &["verify", &id, "--db", "/tmp/whatever.db"]);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("--db"), "{err}");

    let (code, _, err) = run_json_in(&scratch, &["verify", &id, "--backend", "postgres"]);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("--backend"), "{err}");
    assert!(
        !err.split(|c: char| !c.is_ascii_digit())
            .any(|part| part.len() == 5 && part.bytes().all(|b| b.is_ascii_digit())),
        "no internal plan number is named: {err}"
    );
}

#[test]
fn a_byte_patched_archive_is_refused_as_corruption() {
    let scratch = Scratch::new("verify-patched");
    let (id, path) = snapshot(&scratch);
    // A lossy UTF-8 conversion plus a string replace cannot reach the manifest:
    // the member sits inside a gzip stream, so the patch lands in the
    // compressed bytes. What this build sees is a damaged archive, and
    // corruption is the refusal that covers it — nothing is reported as
    // verified.
    let raw = std::fs::read(&path).expect("read the archive");
    let patched = String::from_utf8_lossy(&raw).replace("\"sqlite\"", "\"postgres\"");
    std::fs::write(&path, patched.as_bytes()).expect("write the patched archive");

    let (code, _, err) = run_json_in(&scratch, &["verify", &id]);

    assert_ne!(code, ExitCode::SUCCESS, "{err}");
    assert!(!err.is_empty(), "{err:?}");
}

#[test]
fn verify_refuses_an_archive_whose_manifest_names_an_unknown_backend() {
    let scratch = Scratch::new("verify-unknown-backend");
    let id = unknown_backend_archive(&scratch);

    let (code, _, err) = run_json_in(&scratch, &["verify", &id]);

    // The manifest validator owns this refusal now: a backend this build
    // cannot name is refused before any member is read, so there is no verdict
    // to report and no payload to inflate.
    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("unknown store.backend"), "{err}");
    assert!(err.contains("cassandra"), "{err}");
    assert!(
        err.contains("this build supports"),
        "the refusal names what this build does support: {err}"
    );
}

/// An archive whose manifest records a backend this build does not know,
/// assembled through the container writer so the manifest is genuine.
///
/// `create_archive` validates the manifest before it writes, so an unknown
/// backend cannot come out of the create path at all: the refusal is the
/// manifest's own, and this is the only way to reach it from outside. The
/// payload is staged bytes rather than a store, so the archive is a real
/// container and no database is involved.
fn unknown_backend_archive(scratch: &Scratch) -> String {
    let backup_dir = scratch.join("backups");
    std::fs::create_dir_all(&backup_dir).expect("create the backup directory");
    let payload = staging::stage_bytes(b"not-a-database", &backup_dir, "payload").expect("stage");
    let manifest = clio_backup::BackupManifest {
        kind: clio_backup::KIND.to_owned(),
        format_version: clio_backup::FORMAT_VERSION,
        created_at: "2026-09-27T00:00:00Z".to_owned(),
        tool_version: "clio 0.1.0".to_owned(),
        store: clio_backup::StoreInfo {
            backend: "cassandra".to_owned(),
            slug: "clio".to_owned(),
            database: "clio".to_owned(),
            schema: None,
        },
        snapshot: clio_backup::SnapshotInfo {
            method: "quorum-read".to_owned(),
            consistency: "eventual".to_owned(),
            started_at: "2026-09-27T00:00:00Z".to_owned(),
            duration_ms: 1,
        },
        payload: clio_backup::PayloadInfo {
            member: "store.db".to_owned(),
            bytes: payload.bytes(),
            sha256: payload.sha256().to_owned(),
            uncompressed_bytes: payload.bytes(),
        },
        counts: clio_backup::CountsInfo { complete: true },
        schema_version: None,
        keys: clio_backup::KeysInfo {
            included: false,
            member: None,
            subjects: 0,
            bytes: None,
            note: String::new(),
        },
        label: None,
        note: None,
        compression: clio_backup::COMPRESSION.to_owned(),
    };
    let mut writer = clio_backup::ArchiveWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .write_manifest(&manifest.to_json_bytes())
        .expect("write the manifest");
    writer
        .add_measured_member("store.db", payload.open().expect("open"), payload.bytes())
        .expect("add the payload");
    let bytes = writer.finish().expect("finish the archive");
    let id = "clio-20260927T000000Z-unknown";
    std::fs::write(
        backup_dir.join(clio_backup::archive_file_name(id)),
        bytes.into_inner(),
    )
    .expect("write the archive");
    id.to_owned()
}

#[test]
fn verify_works_with_the_store_down() {
    let scratch = Scratch::new("verify-store-down");
    let (id, _) = snapshot(&scratch);
    std::fs::remove_file(scratch.join("clio.db")).expect("remove the store");
    let (code, payload, err) = run_json_in_with(
        &scratch,
        &["verify", &id],
        &[(
            "CLIO_DATABASE_URL",
            "postgres://clio:clio@127.0.0.1:9/nosuchdb",
        )],
    );

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["ok"], json!(true), "{payload}");
}

#[test]
fn verify_leaves_no_extracted_payload_behind() {
    let scratch = Scratch::new("verify-cleanup");
    let (id, _) = snapshot(&scratch);

    let (code, _, err) = run_json_in(&scratch, &["verify", &id]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");

    let temporaries: Vec<String> = std::fs::read_dir(scratch.join("backups"))
        .expect("read the backup directory")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("verify-payload"))
        .collect();
    assert!(temporaries.is_empty(), "{temporaries:?}");
}

#[test]
fn verify_help_documents_the_selector() {
    let scratch = Scratch::new("verify-help");

    let (code, out, _) = run_in(&scratch, &["verify", "--help"], false);

    assert_eq!(code, ExitCode::SUCCESS);
    assert!(out.contains("usage: clio backup verify"), "{out}");
    assert!(out.contains("--latest"), "{out}");
}

#[test]
fn verify_with_a_failed_current_directory_is_an_internal_error() {
    let error = CliError::operational("internal", "cannot determine the current directory: boom");

    let (code, _, err) = run_backup(&["verify"], &[], Err(error), false);

    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("current directory"), "{err}");
}

#[test]
fn verify_refuses_an_unknown_flag_before_anything_else() {
    let scratch = Scratch::new("verify-bad-flag");
    let cwd = Ok(scratch.path().to_path_buf());

    let (code, _, err) = run_backup(&["verify", "--nonsense"], &[], cwd, false);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("unknown flag"), "{err}");
}

#[test]
fn verify_refuses_an_output_mode_it_does_not_know() {
    let scratch = Scratch::new("verify-bad-output");
    let cwd = Ok(scratch.path().to_path_buf());

    let (code, _, err) = run_backup(&["verify", "--output", "nonsense"], &[], cwd, false);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("--output must be"), "{err}");
}

#[test]
fn latest_with_no_backup_directory_at_all_is_not_found() {
    let scratch = Scratch::new("verify-no-dir");

    let (code, _, err) = run_json_in(&scratch, &["verify", "--latest"]);

    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("no snapshot to verify"), "{err}");
}

#[test]
fn a_backup_path_that_is_a_file_refuses_the_listing() {
    let scratch = Scratch::new("verify-blocked");
    let blocker = scratch.join("backups");
    std::fs::write(&blocker, b"not a directory").expect("write the blocker file");
    let data_dir = scratch.path().display().to_string();
    let blocker_dir = blocker.display().to_string();
    let pairs = [
        ("CLIO_DATA_DIR", data_dir.as_str()),
        ("CLIO_BACKUP_DIR", blocker_dir.as_str()),
    ];

    let (code, _, err) = run_backup(
        &["verify", "--latest", "--output", "json"],
        &pairs,
        Ok(scratch.path().to_path_buf()),
        false,
    );

    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("reading"), "{err}");
    assert!(err.contains("backups"), "{err}");
}

#[test]
fn an_unknown_id_without_a_close_match_points_at_the_listing() {
    let scratch = Scratch::new("verify-unknown-far");
    let (id, _) = snapshot(&scratch);

    let (code, _, err) = run_json_in(&scratch, &["verify", "zzzz"]);

    assert_eq!(code, ExitCode::from(1), "{err}");
    assert!(err.contains("no snapshot `zzzz`"), "{err}");
    assert!(err.contains("clio backup list"), "{err}");
    assert!(!err.contains(&id), "no real id is echoed: {err}");
}
