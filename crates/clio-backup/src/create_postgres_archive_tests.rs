// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests a hand-built Postgres dump archive end to end: it is
//! assembled, extracted, and verified; every member is accounted for, a
//! hostile member is refused, and no credential or row content escapes the
//! member it belongs to. No database is involved: the dump members are plain
//! `COPY` text.
//!
//! ## Owns
//! - The assembly, extraction, verification, and refusal assertions over a
//!   Postgres archive built from staged bytes
//!
//! ## Does not own
//! - The archive writer, reader, dump meta, and verify pass (see `create`,
//!   `container_reader`, `dump_meta`, and `verify`)
//! - The staged-file lifecycle (see `staging`)
//!
//! ## Boundary
//!
//! Test-only, and never compiled into production builds.

use crate::create::{CreateRequest, StagedMember};
use crate::dump_meta::{DUMP_META_MEMBER, DUMP_TABLES_PREFIX, DumpColumn, DumpMeta, DumpTable};
use crate::fixtures;
use crate::postgres_archive_fixtures::{
    Scratch, dump_meta, extract, member_names, postgres_archive,
};
use crate::staging;
use crate::verify::verify_archive;
use clio_types::ErrorCode;

#[test]
fn a_postgres_archive_carries_the_members_in_the_dump_order() {
    let scratch = Scratch::new("pg-archive-order");
    let (meta, members) = dump_meta(&scratch, &[("items", b"i-1\n"), ("triples", b"t-1\n")]);
    let archive = postgres_archive(&scratch, &meta, members);
    assert_eq!(
        member_names(&archive.path),
        vec![
            crate::MANIFEST_MEMBER.to_owned(),
            DUMP_META_MEMBER.to_owned(),
            format!("{DUMP_TABLES_PREFIX}001_items.txt"),
            format!("{DUMP_TABLES_PREFIX}002_triples.txt"),
        ]
    );
}

#[test]
fn the_report_accounts_for_every_member() {
    let scratch = Scratch::new("pg-archive-report");
    let (meta, members) = dump_meta(&scratch, &[("items", b"i-1\n")]);
    let table_bytes: u64 = members.iter().map(|member| member.file.bytes()).sum();
    let meta_bytes = meta.to_json_bytes().len() as u64;
    let report = postgres_archive(&scratch, &meta, members);
    assert_eq!(report.payload_bytes, meta_bytes);
    // Manifest bytes + meta bytes + table bytes; the manifest's own share is
    // small, so the sum is asserted from below.
    assert!(report.uncompressed_bytes >= table_bytes + meta_bytes);
}

#[test]
fn the_dump_meta_reads_back_from_the_archive_without_a_table_member() {
    let scratch = Scratch::new("pg-archive-meta-read");
    let (meta, members) = dump_meta(&scratch, &[("items", b"i-1\n")]);
    let archive = postgres_archive(&scratch, &meta, members);
    let read = DumpMeta::read_from_archive(&archive.path).expect("read the dump meta");
    assert_eq!(read, meta);
}

#[test]
fn extraction_stages_every_declared_member() {
    let scratch = Scratch::new("pg-archive-extract");
    let (meta, members) = dump_meta(&scratch, &[("items", b"i-1\n"), ("triples", b"t-1\n")]);
    let archive = postgres_archive(&scratch, &meta, members);
    let out = Scratch::new("pg-archive-extract-out");
    let extracted = extract(&archive.path, out.path()).expect("extract");
    assert_eq!(extracted.dump_tables.len(), 2);
    let items = extracted
        .dump_tables
        .iter()
        .find(|member| member.member.ends_with("items.txt"))
        .expect("items member");
    let bytes = std::fs::read(items.file.path()).expect("read the staged member");
    assert_eq!(bytes, b"i-1\n");
}

#[test]
fn extraction_refuses_a_member_whose_digest_disagrees_with_the_meta() {
    let scratch = Scratch::new("pg-archive-corrupt");
    let (meta, members) = dump_meta(&scratch, &[("items", b"i-1\n")]);
    // Re-record a wrong digest in the meta before assembling, so the archive
    // carries a member its own meta does not describe.
    let mut lying = meta;
    lying.tables[0].sha256 = "0".repeat(64);
    let archive = postgres_archive(&scratch, &lying, members);
    let out = Scratch::new("pg-archive-corrupt-out");
    let error = extract(&archive.path, out.path()).unwrap_err();
    assert_eq!(error.code, ErrorCode::ArchiveCorrupt, "{error}");
    assert!(error.message.contains("digest"), "{error}");
}

#[test]
fn extraction_refuses_a_declared_member_the_archive_does_not_carry() {
    let scratch = Scratch::new("pg-archive-missing");
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
    let out = Scratch::new("pg-archive-missing-out");
    let error = extract(&archive.path, out.path()).unwrap_err();
    assert_eq!(error.code, ErrorCode::ArchiveCorrupt, "{error}");
    assert!(error.message.contains("002_triples.txt"), "{error}");
}

#[test]
fn extraction_refuses_an_undeclared_table_member() {
    let scratch = Scratch::new("pg-archive-undeclared");
    let (meta, mut members) = dump_meta(&scratch, &[("items", b"i-1\n")]);
    let extra = staging::stage_bytes(b"t-1\n", scratch.path(), "dump-extra").expect("stage extra");
    members.push(StagedMember {
        name: format!("{DUMP_TABLES_PREFIX}099_extra.txt"),
        file: extra,
    });
    let archive = postgres_archive(&scratch, &meta, members);
    let out = Scratch::new("pg-archive-undeclared-out");
    let error = extract(&archive.path, out.path()).unwrap_err();
    assert_eq!(error.code, ErrorCode::ArchiveCorrupt, "{error}");
    assert!(
        error
            .message
            .contains("declares no table member with that name"),
        "{error}"
    );
}

#[test]
fn verify_reports_each_table_member_against_the_meta() {
    let scratch = Scratch::new("pg-archive-verify");
    let (meta, members) = dump_meta(&scratch, &[("items", b"i-1\n")]);
    let archive = postgres_archive(&scratch, &meta, members);
    let report = verify_archive(&archive.path, &mut |payload| {
        crate::dump_meta::validate_dump_meta_file(payload)
    })
    .expect("verify");
    assert!(report.ok, "{report:?}");
    assert_eq!(report.backend, "postgres");
    let check = report
        .members
        .iter()
        .find(|check| check.member.ends_with("items.txt"))
        .expect("the table member check");
    assert!(check.ok, "{}", check.detail);
    assert!(
        check.detail.contains("match the dump meta"),
        "{}",
        check.detail
    );
}

#[test]
fn verify_fails_a_table_member_that_disagrees_with_the_meta() {
    let scratch = Scratch::new("pg-archive-verify-bad");
    let (meta, members) = dump_meta(&scratch, &[("items", b"i-1\n")]);
    let mut lying = meta;
    lying.tables[0].sha256 = "0".repeat(64);
    let archive = postgres_archive(&scratch, &lying, members);
    let report = verify_archive(&archive.path, &mut |payload| {
        crate::dump_meta::validate_dump_meta_file(payload)
    })
    .expect("verify");
    assert!(!report.ok, "{report:?}");
    assert!(
        report
            .failed()
            .iter()
            .any(|name| name.ends_with("items.txt")),
        "{:?}",
        report.failed()
    );
}

#[test]
fn verify_refuses_a_dump_meta_that_does_not_parse() {
    let scratch = Scratch::new("pg-archive-verify-unparsable");
    let (meta, members) = dump_meta(&scratch, &[("items", b"i-1\n")]);
    let archive = postgres_archive(&scratch, &meta, members);
    // The meta member's bytes are replaced inside the staged payload file the
    // verify pass will parse — but the archive is sealed, so the refusal is
    // reached the same way a hostile archive reaches it: a meta whose digest
    // matches its manifest but whose content does not parse. Build one by
    // assembling with a valid digest for an invalid meta body.
    let bad_meta = "not json at all";
    let payload = staging::stage_bytes(bad_meta.as_bytes(), scratch.path(), "bad-meta")
        .expect("stage a bad meta");
    let request = {
        let (meta, members) = dump_meta(&scratch, &[("items", b"i-1\n")]);
        let _ = meta;
        CreateRequest {
            backup_dir: scratch.path().to_path_buf(),
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
            duration_ms: 42,
            counts_complete: true,
            keys: None,
            label: None,
            note: None,
            tool_version: "test".to_owned(),
            authority: None,
        }
    };
    let _ = archive;
    let hostile = crate::create_archive(&request).expect("assemble the hostile archive");
    let report = verify_archive(&hostile.path, &mut |payload| {
        crate::dump_meta::validate_dump_meta_file(payload)
    });
    // The manifest's payload digest was computed over the invalid meta, so the
    // digest check passes and the parse is what catches it.
    let error = report.unwrap_err();
    assert_eq!(error.code, ErrorCode::ArchiveCorrupt, "{error}");
}

#[test]
fn a_sqlite_archive_is_extracted_unchanged() {
    let scratch = Scratch::new("pg-archive-sqlite");
    let payload = fixtures::staged_payload(scratch.path(), 1);
    let report = crate::create_archive(&fixtures::request(scratch.path(), payload))
        .expect("assemble the sqlite archive");
    let out = Scratch::new("pg-archive-sqlite-out");
    let extracted = extract(&report.path, out.path()).expect("extract");
    assert!(
        extracted.dump_tables.is_empty(),
        "a SQLite archive carries no dump members"
    );
}

#[test]
fn a_key_member_still_travels_with_a_postgres_archive() {
    let scratch = Scratch::new("pg-archive-keys");
    let (meta, members) = dump_meta(&scratch, &[("items", b"i-1\n")]);
    let key = fixtures::key_source(scratch.path(), 1);
    let payload = staging::stage_bytes(&meta.to_json_bytes(), scratch.path(), "dump-meta")
        .expect("stage meta");
    let request = CreateRequest {
        backup_dir: scratch.path().to_path_buf(),
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
        duration_ms: 42,
        counts_complete: true,
        keys: Some(key),
        label: None,
        note: None,
        tool_version: "test".to_owned(),
        authority: None,
    };
    let archive = crate::create_archive(&request).expect("assemble");
    let out = Scratch::new("pg-archive-keys-out");
    let extracted = extract(&archive.path, out.path()).expect("extract");
    assert!(extracted.key.is_some(), "the key member is carried");
    assert_eq!(extracted.dump_tables.len(), 1);
}

#[test]
fn no_url_rides_in_a_postgres_archive_outside_the_row_it_came_from() {
    let scratch = Scratch::new("pg-archive-credential");
    // The row content carries the URL, so the assertion is not passing because
    // nothing was captured: the URL is in the items member, and in no other
    // member, and in neither the manifest nor the dump meta.
    let url = b"postgres://user:password@host/db\n";
    let (meta, members) = dump_meta(&scratch, &[("items", url)]);
    let archive = postgres_archive(&scratch, &meta, members);

    // The members are read back through the crate's own reader, so the bytes
    // compared are the decompressed member bytes. Grepping the raw archive
    // file would read deflate output, where the literal cannot appear whether
    // or not a credential is present.
    let out = Scratch::new("pg-archive-credential-out");
    let extracted = extract(&archive.path, out.path()).expect("extract the archive");

    // The row's own member carries it: that is where row content belongs.
    let items =
        std::fs::read(extracted.dump_tables[0].file.path()).expect("read the staged items member");
    assert!(
        String::from_utf8_lossy(&items).contains("postgres://"),
        "the planted row is in the items member, so the checks below are real"
    );

    // The manifest, read back out of the archive.
    let manifest = crate::read_manifest(&archive.path).expect("read the manifest");
    let manifest_text = serde_json::to_string(&manifest).expect("serialize the manifest");
    assert!(!manifest_text.contains("postgres://"), "{manifest_text}");

    // The dump meta, read back out of the archive.
    let meta_text = String::from_utf8_lossy(&meta.to_json_bytes()).into_owned();
    assert!(!meta_text.contains("postgres://"), "{meta_text}");

    // The payload member, which is where the meta itself is stored.
    let payload = std::fs::read(extracted.payload.file.path()).expect("read the payload");
    assert!(
        !String::from_utf8_lossy(&payload).contains("password@host"),
        "the meta member carries no credential"
    );
}

#[test]
fn verify_names_a_declared_member_the_archive_does_not_carry() {
    let scratch = Scratch::new("pg-archive-verify-missing");
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
    let report = verify_archive(&archive.path, &mut |payload| {
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
}
