// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests one backup directory holding both backends' archives: the
//! listing names each row's backend, and verify passes each archive with its
//! own method.
//!
//! ## Owns
//! - The mixed-directory listing assertions and the per-backend verify
//!   assertions, including the method, consistency, and schema each report
//!
//! ## Does not own
//! - The `list` and `verify` commands themselves (see `backup_list_cli` and
//!   `backup_verify_cli`)
//! - The archive writer and the Postgres dump (see `clio-backup` and
//!   `clio-store`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::backup_cli_fixtures::{Scratch, populated_store};
use clio_backup::create::{CreateRequest, StagedMember};
use clio_backup::dump_meta::{
    DUMP_FORMAT_VERSION, DUMP_KIND, DUMP_META_MEMBER, DUMP_TABLES_PREFIX, DumpColumn, DumpMeta,
    DumpTable,
};
use clio_backup::staging;
use serde_json::json;
use std::path::Path;

/// Assemble one Postgres archive into `scratch`'s backup directory without a
/// database: the dump members are plain `COPY` text and the meta records them.
fn postgres_archive(scratch: &Scratch) -> String {
    let backup_dir = scratch.join("backups");
    std::fs::create_dir_all(&backup_dir).expect("backup dir");
    let rows: &[(&str, &[u8])] = &[("items", b"i-1\n")];
    let mut tables = Vec::new();
    let mut members = Vec::new();
    for (index, (table, body)) in rows.iter().enumerate() {
        let member = format!("{DUMP_TABLES_PREFIX}{:03}_{table}.txt", index + 1);
        let staged = staging::stage_bytes(body, &backup_dir, "dump-table").expect("stage rows");
        tables.push(DumpTable {
            table: (*table).to_owned(),
            member: member.clone(),
            bytes: staged.bytes(),
            sha256: staged.sha256().to_owned(),
            rows: 1,
            columns: vec![DumpColumn {
                name: "id".to_owned(),
                ordinal: 1,
                type_name: "text".to_owned(),
            }],
        });
        members.push(StagedMember {
            name: member,
            file: staged,
        });
    }
    let meta = DumpMeta {
        kind: DUMP_KIND.to_owned(),
        format_version: DUMP_FORMAT_VERSION,
        backend: "postgres".to_owned(),
        schema: "am_e2e_dev".to_owned(),
        database: "clio".to_owned(),
        extensions: vec!["vector".to_owned()],
        banks: vec!["demo".to_owned()],
        method: "postgres-copy".to_owned(),
        consistency: "point-in-time".to_owned(),
        tables,
    };
    let payload =
        staging::stage_bytes(&meta.to_json_bytes(), &backup_dir, "dump-meta").expect("stage meta");
    let request = CreateRequest {
        backup_dir,
        slug: "clio".to_owned(),
        backend: "postgres".to_owned(),
        database: "clio".to_owned(),
        store_schema: Some("am_e2e_dev".to_owned()),
        schema_version: Some("1".to_owned()),
        payload,
        payload_member: DUMP_META_MEMBER.to_owned(),
        additional_members: members,
        method: "postgres-copy".to_owned(),
        consistency: "point-in-time".to_owned(),
        started_at: "2026-09-27T00:00:00Z".to_owned(),
        duration_ms: 42,
        counts_complete: true,
        keys: None,
        label: Some("pg-handbuilt".to_owned()),
        note: None,
        tool_version: "test".to_owned(),
    };
    let report = clio_backup::create_archive(&request).expect("assemble the archive");
    report.id
}

/// A SQLite archive in the same directory, through the real create.
fn sqlite_archive(scratch: &Scratch) -> String {
    populated_store(&scratch.join("clio.db"), 1, 1);
    let (code, payload, err) = crate::backup_cli_fixtures::run_json_in(scratch, &["create"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    payload["snapshot_id"].as_str().expect("an id").to_owned()
}

#[test]
fn a_listing_holding_both_backends_states_each_row_s_backend() {
    let scratch = Scratch::new("list-mixed");
    let sqlite_id = sqlite_archive(&scratch);
    let postgres_id = postgres_archive(&scratch);

    let (code, payload, err) = crate::backup_cli_fixtures::run_json_in(&scratch, &["list"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["count"], json!(2), "{payload}");
    let rows = payload["snapshots"].as_array().expect("rows");
    let backends: Vec<&str> = rows
        .iter()
        .map(|row| row["backend"].as_str().expect("a backend"))
        .collect();
    assert!(backends.contains(&"sqlite"), "{rows:?}");
    assert!(backends.contains(&"postgres"), "{rows:?}");
    for row in rows {
        assert!(row["unreadable"].is_null(), "{row:?}");
    }
    let _ = (&sqlite_id, &postgres_id);
}

#[test]
fn verify_passes_each_backend_s_archive_and_reports_its_method() {
    let scratch = Scratch::new("verify-mixed");
    let _sqlite_id = sqlite_archive(&scratch);
    let postgres_id = postgres_archive(&scratch);

    // The Postgres archive verifies: every member against its own record, and
    // the report states how this backend was copied and which schema it was
    // scoped to — a Postgres dump is schema-scoped, so a report that named the
    // backend alone would not say which schema was checked.
    let (code, payload, err) =
        crate::backup_cli_fixtures::run_json_in(&scratch, &["verify", &postgres_id]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert_eq!(payload["backend"], json!("postgres"), "{payload}");
    assert_eq!(payload["method"], json!("postgres-copy"), "{payload}");
    assert_eq!(payload["consistency"], json!("point-in-time"), "{payload}");
    assert_eq!(payload["schema"], json!("am_e2e_dev"), "{payload}");
    let members = payload["members"].as_array().expect("members");
    assert!(
        members
            .iter()
            .any(|check| check["member"] == json!(DUMP_META_MEMBER)),
        "{members:?}"
    );
    assert!(
        members.iter().any(|check| check["member"]
            .as_str()
            .unwrap_or("")
            .ends_with("items.txt")),
        "{members:?}"
    );

    // The SQLite archive still verifies the way it always did, and states the
    // `VACUUM INTO` method that took it. SQLite has no schema of its own, so
    // the report's schema is null rather than an empty string.
    let rows = clio_backup::list_snapshot_rows(&scratch.join("backups"), None).expect("rows");
    let sqlite_id = rows
        .iter()
        .find(|row| row.backend == "sqlite")
        .expect("a sqlite archive")
        .id
        .clone();
    let (code, payload, err) =
        crate::backup_cli_fixtures::run_json_in(&scratch, &["verify", &sqlite_id]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert_eq!(payload["backend"], json!("sqlite"), "{payload}");
    assert_eq!(payload["method"], json!("vacuum-into"), "{payload}");
    assert_eq!(payload["consistency"], json!("point-in-time"), "{payload}");
    assert!(payload["schema"].is_null(), "{payload}");
    let _ = Path::new("");
}
