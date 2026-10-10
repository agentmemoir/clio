// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the verify pass's edge refusals, the payload check's
//! detail reporting, and the facts a report must carry: a missing archive, a
//! manifest that records the right digest but the wrong size, the reported
//! backend, method, consistency and store schema, and a Postgres dumped table
//! member the dump meta does not declare.
//!
//! ## Owns
//! - The missing-archive refusal and the size-only-mismatch detail assertions
//! - The reported backend, method, consistency, and store schema
//! - The Postgres table-member mirror of the extraction refusal: an undeclared
//!   member fails the pass and is listed as unlisted
//!
//! ## Does not own
//! - The per-member verify rules (see `verify_tests`)
//! - The Postgres archive assembly and extraction (see `postgres_archive_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::create::{CreateRequest, StagedMember};
use crate::dump_meta::{
    DUMP_FORMAT_VERSION, DUMP_KIND, DUMP_META_MEMBER, DUMP_TABLES_PREFIX, DumpColumn, DumpMeta,
    DumpTable,
};
use crate::fixtures::{Scratch, staged_payload};
use crate::staging;
use std::path::Path;

#[test]
fn verify_archive_refuses_a_missing_file() {
    let error = super::verify_archive(
        Path::new("/nonexistent/clio/no-such-archive.cliobak.tar.gz"),
        &mut |_| Ok(()),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound, "{error}");
}

#[test]
fn the_payload_check_reports_a_size_only_mismatch_in_its_detail() {
    let scratch = Scratch::new("verify-payload-size-lie");
    let staged = staged_payload(scratch.path(), 1);
    // A manifest that records the right digest but the wrong size: the check
    // must fail and name the size difference.
    let lying = manifest_with_size(&staged.bytes() + 1, staged.sha256());
    let check = super::payload_check(&lying, &staged);
    assert!(!check.ok, "{}", check.detail);
    assert!(
        check.detail.contains("the manifest records"),
        "{}",
        check.detail
    );
    assert!(
        check.detail.contains("the member holds"),
        "{}",
        check.detail
    );
}

/// A minimal manifest whose payload record lies, for the payload_check test.
fn manifest_with_size(bytes: u64, sha256: &str) -> crate::manifest::BackupManifest {
    crate::manifest::BackupManifest {
        kind: crate::manifest::KIND.to_owned(),
        format_version: crate::manifest::FORMAT_VERSION,
        created_at: "2026-09-27T00:00:00Z".to_owned(),
        tool_version: "test".to_owned(),
        store: crate::manifest::StoreInfo {
            backend: "sqlite".to_owned(),
            slug: "clio".to_owned(),
            database: "clio.db".to_owned(),
            schema: None,
        },
        snapshot: crate::manifest::SnapshotInfo {
            method: "vacuum-into".to_owned(),
            consistency: "point-in-time".to_owned(),
            started_at: "2026-09-27T00:00:00Z".to_owned(),
            duration_ms: 1,
        },
        payload: crate::manifest::PayloadInfo {
            member: "store.db".to_owned(),
            bytes,
            sha256: sha256.to_owned(),
            uncompressed_bytes: bytes,
        },
        counts: crate::manifest::CountsInfo { complete: true },
        schema_version: None,
        keys: crate::manifest::KeysInfo {
            included: false,
            member: None,
            subjects: 0,
            bytes: None,
            note: String::new(),
        },
        label: None,
        note: None,
        compression: crate::manifest::COMPRESSION.to_owned(),
    }
}

#[test]
fn the_report_states_the_backend_and_the_method_from_the_manifest() {
    let scratch = Scratch::new("verify-report-facts");
    let staged = staged_payload(scratch.path(), 3);
    // A hand-picked method and consistency, so the report can only be showing
    // what the manifest recorded rather than a constant. The create builds the
    // manifest from the request, so the request is where these facts are set.
    let mut request = crate::fixtures::request(scratch.path(), staged);
    request.method = "sqlite-backup-api".to_owned();
    request.consistency = "quorum-consistent".to_owned();
    let created = crate::create::create_archive(&request).expect("create the archive");

    let report = super::verify_archive(&created.path, &mut |_| Ok(())).expect("verify");

    assert!(report.ok, "failed checks: {:?}", report.failed());
    assert_eq!(report.backend, "sqlite");
    assert_eq!(report.method, "sqlite-backup-api");
    assert_eq!(report.consistency, "quorum-consistent");
}

#[test]
fn a_backend_without_a_schema_reports_none_rather_than_an_empty_string() {
    let scratch = Scratch::new("verify-report-no-schema");
    let staged = staged_payload(scratch.path(), 4);
    let request = crate::fixtures::request(scratch.path(), staged);
    let created = crate::create::create_archive(&request).expect("create the archive");

    let report = super::verify_archive(&created.path, &mut |_| Ok(())).expect("verify");

    assert_eq!(report.backend, "sqlite");
    assert_eq!(report.method, "vacuum-into");
    // A SQLite store has no schema of its own, and an absent value is not the
    // same statement as an empty one: `None` says the manifest recorded none.
    assert_eq!(report.store_schema, None);
}

/// A dump meta over `tables`, one row per table in name order, with the staged
/// member's own measured digest recorded.
fn dump_meta(scratch: &Scratch, tables: &[(&str, &[u8])]) -> (DumpMeta, Vec<StagedMember>) {
    let mut entries = Vec::new();
    let mut members = Vec::new();
    for (index, (table, rows)) in tables.iter().enumerate() {
        let member = format!("{DUMP_TABLES_PREFIX}{:03}_{table}.txt", index + 1);
        let staged = staging::stage_bytes(rows, scratch.path(), "dump-table").expect("stage rows");
        entries.push(DumpTable {
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
        schema: "public".to_owned(),
        database: "clio".to_owned(),
        extensions: vec!["vector".to_owned()],
        banks: vec!["default".to_owned()],
        method: "postgres-copy".to_owned(),
        consistency: "point-in-time".to_owned(),
        tables: entries,
    };
    (meta, members)
}

/// Assemble a Postgres archive from a meta and its table members.
fn postgres_archive(
    scratch: &Scratch,
    meta: &DumpMeta,
    members: Vec<StagedMember>,
) -> crate::create::CreateReport {
    let payload = staging::stage_bytes(&meta.to_json_bytes(), scratch.path(), "dump-meta")
        .expect("stage the dump meta");
    let request = CreateRequest {
        backup_dir: scratch.path().to_path_buf(),
        slug: "clio".to_owned(),
        backend: "postgres".to_owned(),
        database: meta.database.clone(),
        store_schema: Some(meta.schema.clone()),
        schema_version: Some("1".to_owned()),
        payload,
        payload_member: DUMP_META_MEMBER.to_owned(),
        additional_members: members,
        method: meta.method.clone(),
        consistency: meta.consistency.clone(),
        started_at: "2026-09-27T00:00:00Z".to_owned(),
        duration_ms: 42,
        counts_complete: true,
        keys: None,
        label: None,
        note: None,
        tool_version: "test".to_owned(),
    };
    crate::create_archive(&request).expect("assemble the archive")
}

/// A dumped table member the dump meta does not declare is the report-side
/// mirror of the extraction refusal: the archive carries rows nothing proved,
/// so the pass must fail, name the member, and list it as unlisted. An
/// extraction refuses the same archive, so a pass that cleared it would vouch
/// for one a restore will not take.
#[test]
fn verify_names_an_undeclared_table_member_and_fails_the_archive() {
    let scratch = Scratch::new("pg-verify-undeclared");
    let (meta, mut members) = dump_meta(&scratch, &[("items", b"i-1\n")]);
    let extra = staging::stage_bytes(b"t-1\n", scratch.path(), "dump-extra").expect("stage extra");
    members.push(StagedMember {
        name: format!("{DUMP_TABLES_PREFIX}099_extra.txt"),
        file: extra,
    });
    let archive = postgres_archive(&scratch, &meta, members);

    let report = super::verify_archive(&archive.path, &mut |payload| {
        crate::dump_meta::validate_dump_meta_file(payload)
    })
    .expect("verify");

    assert!(!report.ok, "an undeclared table member fails the pass");
    let undeclared = report
        .members
        .iter()
        .find(|check| check.member.ends_with("099_extra.txt"))
        .expect("the undeclared member has a check of its own");
    assert!(!undeclared.ok, "{}", undeclared.detail);
    assert!(
        undeclared.detail.contains("declares no such table member"),
        "{}",
        undeclared.detail
    );
    assert!(
        report
            .unlisted
            .iter()
            .any(|name| name.ends_with("099_extra.txt")),
        "the undeclared member is listed: {:?}",
        report.unlisted
    );
    assert!(
        report
            .failed()
            .iter()
            .any(|name| name.ends_with("099_extra.txt")),
        "the undeclared member is named as a failure: {:?}",
        report.failed()
    );
    // The declared member still passes, so the failure is the undeclared one
    // alone and not a side effect of the whole table comparison.
    let declared = report
        .members
        .iter()
        .find(|check| check.member.ends_with("001_items.txt"))
        .expect("the declared member's check");
    assert!(declared.ok, "{}", declared.detail);
    // The extraction half refuses this archive with its own message, so the
    // two halves agree on what the archive is.
    let out = Scratch::new("pg-verify-undeclared-out");
    let manifest = crate::inventory::read_manifest(&archive.path).expect("read the manifest");
    let error = crate::extract::extract_members(&archive.path, &manifest, out.path()).unwrap_err();
    assert_eq!(error.code, ErrorCode::ArchiveCorrupt, "{error}");
    assert!(
        error
            .message
            .contains("declares no table member with that name"),
        "{error}"
    );
}

/// A dump meta that declares a table the archive does not carry is a damaged
/// dump: the pass must fail on the missing member, and the member is not an
/// unlisted one, because the meta is what declares it.
#[test]
fn verify_fails_a_table_member_the_archive_does_not_carry() {
    let scratch = Scratch::new("pg-verify-missing");
    let (mut meta, members) = dump_meta(&scratch, &[("items", b"i-1\n")]);
    meta.tables.push(DumpTable {
        table: "triples".to_owned(),
        member: format!("{DUMP_TABLES_PREFIX}002_triples.txt"),
        bytes: 4,
        sha256: "a".repeat(64),
        rows: 1,
        columns: vec![DumpColumn {
            name: "id".to_owned(),
            ordinal: 1,
            type_name: "text".to_owned(),
        }],
    });
    let archive = postgres_archive(&scratch, &meta, members);

    let report = super::verify_archive(&archive.path, &mut |payload| {
        crate::dump_meta::validate_dump_meta_file(payload)
    })
    .expect("verify");

    assert!(!report.ok, "{report:?}");
    let missing = report
        .members
        .iter()
        .find(|check| check.member.ends_with("002_triples.txt"))
        .expect("the missing member's check");
    assert!(!missing.ok, "{}", missing.detail);
    assert!(missing.detail.contains("the archive has no such member"));
    assert!(
        !report
            .unlisted
            .iter()
            .any(|name| name.ends_with("002_triples.txt")),
        "a declared member is not an unlisted one: {:?}",
        report.unlisted
    );
}
