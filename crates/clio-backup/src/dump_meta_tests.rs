// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `dump/meta.json` member: its round trip, its
//! validation refusals, and its deterministic serialization.
//!
//! ## Owns
//! - The serialization round trip, the ordering and naming rules, and every
//!   refusal a malformed or hostile meta must draw
//!
//! ## Does not own
//! - The meta itself and its reader (see `dump_meta`)
//! - The Postgres archive that carries it (see `postgres_archive_tests`)
//!
//! ## Boundary
//!
//! Test-only, and never compiled into production builds.

use super::*;
use clio_types::ErrorCode;

/// One valid dump meta, for the tests to mutate.
fn valid_meta() -> DumpMeta {
    DumpMeta {
        kind: DUMP_KIND.to_owned(),
        format_version: DUMP_FORMAT_VERSION,
        backend: "postgres".to_owned(),
        schema: "public".to_owned(),
        database: "clio".to_owned(),
        extensions: vec!["plpgsql".to_owned(), "vector".to_owned()],
        banks: vec!["default".to_owned()],
        method: "postgres-copy".to_owned(),
        consistency: "point-in-time".to_owned(),
        tables: vec![DumpTable {
            table: "items".to_owned(),
            member: "dump/tables/001_items.txt".to_owned(),
            bytes: 12,
            sha256: "a".repeat(64),
            rows: 2,
            columns: vec![DumpColumn {
                name: "id".to_owned(),
                ordinal: 1,
                type_name: "text".to_owned(),
            }],
        }],
    }
}

#[test]
fn round_trips_through_json() {
    let meta = valid_meta();
    let bytes = meta.to_json_bytes();
    let parsed = DumpMeta::from_json_bytes(&bytes).unwrap();
    assert_eq!(parsed, meta);
}

#[test]
fn serializes_deterministically() {
    let mut a = valid_meta();
    let mut b = valid_meta();
    // Field values that differ do not change the field order, and equal metas
    // serialize to identical bytes.
    a.database = "other".to_owned();
    b.database = "other".to_owned();
    assert_eq!(a.to_json_bytes(), b.to_json_bytes());
    let text = String::from_utf8(valid_meta().to_json_bytes()).unwrap();
    assert!(text.starts_with(r#"{"kind":"clio-store-dump""#));
}

#[test]
fn ignores_unknown_fields_on_read() {
    let mut json = serde_json::to_value(valid_meta()).unwrap();
    json["a_field_a_future_version_adds"] = serde_json::json!(1);
    let bytes = serde_json::to_vec(&json).unwrap();
    assert!(DumpMeta::from_json_bytes(&bytes).is_ok());
}

#[test]
fn refuses_a_wrong_kind_or_version_or_backend() {
    for change in ["kind", "format_version", "backend"] {
        let mut meta = valid_meta();
        match change {
            "kind" => meta.kind = "other".to_owned(),
            "format_version" => meta.format_version = 99,
            _ => meta.backend = "sqlite".to_owned(),
        }
        assert_eq!(
            meta.validate().unwrap_err().code,
            ErrorCode::ArchiveCorrupt,
            "{change}"
        );
    }
}

#[test]
fn refuses_an_empty_required_field() {
    for field in ["schema", "database", "method", "consistency"] {
        let mut meta = valid_meta();
        match field {
            "schema" => meta.schema = String::new(),
            "database" => meta.database = String::new(),
            "method" => meta.method = String::new(),
            _ => meta.consistency = String::new(),
        }
        assert_eq!(
            meta.validate().unwrap_err().code,
            ErrorCode::ArchiveCorrupt,
            "{field}"
        );
    }
}

#[test]
fn refuses_an_empty_table_set() {
    let mut meta = valid_meta();
    meta.tables.clear();
    assert_eq!(meta.validate().unwrap_err().code, ErrorCode::ArchiveCorrupt);
}

#[test]
fn refuses_tables_out_of_name_order() {
    let mut meta = valid_meta();
    meta.tables.push(DumpTable {
        table: "assoc_edges".to_owned(),
        member: "dump/tables/002_assoc_edges.txt".to_owned(),
        bytes: 0,
        sha256: "b".repeat(64),
        rows: 0,
        columns: meta.tables[0].columns.clone(),
    });
    assert_eq!(meta.validate().unwrap_err().code, ErrorCode::ArchiveCorrupt);
    // Ascending is fine.
    let mut meta = valid_meta();
    let later = DumpTable {
        table: "triples".to_owned(),
        member: "dump/tables/002_triples.txt".to_owned(),
        bytes: 0,
        sha256: "c".repeat(64),
        rows: 0,
        columns: meta.tables[0].columns.clone(),
    };
    meta.tables.push(later);
    assert!(meta.validate().is_ok());
}

#[test]
fn refuses_a_table_member_name_outside_the_numbering() {
    let mut meta = valid_meta();
    meta.tables[0].member = "elsewhere.txt".to_owned();
    assert_eq!(meta.validate().unwrap_err().code, ErrorCode::ArchiveCorrupt);
}

#[test]
fn refuses_a_digest_that_is_not_lowercase_hex() {
    let mut meta = valid_meta();
    meta.tables[0].sha256 = "A".repeat(64);
    assert_eq!(meta.validate().unwrap_err().code, ErrorCode::ArchiveCorrupt);
    meta.tables[0].sha256 = "a".repeat(63);
    assert_eq!(meta.validate().unwrap_err().code, ErrorCode::ArchiveCorrupt);
}

#[test]
fn refuses_columns_that_are_empty_or_out_of_ordinal_order() {
    let mut meta = valid_meta();
    meta.tables[0].columns.clear();
    assert_eq!(meta.validate().unwrap_err().code, ErrorCode::ArchiveCorrupt);
    let mut meta = valid_meta();
    meta.tables[0].columns.push(DumpColumn {
        name: "later".to_owned(),
        ordinal: 1,
        type_name: "text".to_owned(),
    });
    assert_eq!(meta.validate().unwrap_err().code, ErrorCode::ArchiveCorrupt);
}

#[test]
fn refuses_a_member_name_the_container_would_reject() {
    let mut meta = valid_meta();
    meta.tables[0].member = "dump/tables/001_a b.txt".to_owned();
    // A space is printable ASCII, so the container accepts it; the numbering
    // prefix and suffix are what the meta requires.
    assert!(meta.validate().is_ok());
    let mut meta = valid_meta();
    meta.tables[0].member = "dump/tables/../etc.txt".to_owned();
    assert_eq!(meta.validate().unwrap_err().code, ErrorCode::ArchiveCorrupt);
}

#[test]
fn table_member_bytes_sums_the_tables() {
    let mut meta = valid_meta();
    meta.tables.push(DumpTable {
        table: "triples".to_owned(),
        member: "dump/tables/002_triples.txt".to_owned(),
        bytes: 5,
        sha256: "d".repeat(64),
        rows: 0,
        columns: meta.tables[0].columns.clone(),
    });
    meta.tables.reverse();
    assert_eq!(meta.table_member_bytes(), 17);
}

/// A meta whose tables both claim `u64::MAX` bytes is refused at parse time.
///
/// The counts come out of the archive, so a wrapped total would be a small
/// number the restore's space check would read as "this dump is tiny", and a
/// debug build would panic on the overflow instead. Neither is acceptable, so
/// the parse refuses a count no reader could ever inflate.
#[test]
fn refuses_a_table_byte_count_the_container_could_never_inflate() {
    let mut meta = valid_meta();
    meta.tables[0].bytes = u64::MAX;
    meta.tables.push(DumpTable {
        table: "triples".to_owned(),
        member: "dump/tables/002_triples.txt".to_owned(),
        bytes: u64::MAX,
        sha256: "d".repeat(64),
        rows: 0,
        columns: meta.tables[0].columns.clone(),
    });
    let error = meta.validate().unwrap_err();
    assert_eq!(error.code, ErrorCode::ArchiveCorrupt, "{error}");
    assert!(error.message.contains("per-member cap"), "{error}");
    // The same meta refused through the public parse, not only through
    // `validate`, so a hostile member is stopped at the door every reader
    // goes through.
    let error = DumpMeta::from_json_bytes(&meta.to_json_bytes()).unwrap_err();
    assert_eq!(error.code, ErrorCode::ArchiveCorrupt, "{error}");
    // The refusal is a bound, not a ban on large tables: a count at the cap is
    // accepted, and one byte over it is not.
    let max = crate::container::ReaderBounds::default().max_member_bytes;
    let mut meta = valid_meta();
    meta.tables[0].bytes = max;
    assert!(meta.validate().is_ok(), "a count at the cap is allowed");
    meta.tables[0].bytes = max + 1;
    assert_eq!(meta.validate().unwrap_err().code, ErrorCode::ArchiveCorrupt);
}

/// The total saturates rather than wrapping, so a caller sizing a restore never
/// sees a small number for a huge declared dump.
#[test]
fn table_member_bytes_saturates_instead_of_wrapping() {
    let mut meta = valid_meta();
    meta.tables[0].bytes = u64::MAX - 4;
    meta.tables.push(DumpTable {
        table: "triples".to_owned(),
        member: "dump/tables/002_triples.txt".to_owned(),
        bytes: 1024,
        sha256: "d".repeat(64),
        rows: 0,
        columns: meta.tables[0].columns.clone(),
    });
    assert_eq!(
        meta.table_member_bytes(),
        u64::MAX,
        "no wrap-around to a small number"
    );
}

#[test]
fn read_from_archive_refuses_an_archive_without_the_member() {
    use crate::staging;
    use crate::{CreateRequest, create_archive};

    let dir = std::env::temp_dir().join(format!("clio-dumpmeta-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let payload = staging::stage_bytes(b"{}", &dir, "test").unwrap();
    let request = CreateRequest {
        backup_dir: dir.clone(),
        slug: "clio".to_owned(),
        backend: "sqlite".to_owned(),
        database: "clio.db".to_owned(),
        store_schema: None,
        schema_version: None,
        payload,
        payload_member: "store.db".to_owned(),
        method: "vacuum-into".to_owned(),
        consistency: "point-in-time".to_owned(),
        started_at: "2026-01-01T00:00:00Z".to_owned(),
        duration_ms: 1,
        counts_complete: true,
        additional_members: Vec::new(),
        keys: None,
        label: None,
        note: None,
        tool_version: "test".to_owned(),
        authority: None,
    };
    let archive = create_archive(&request).unwrap();
    let error = DumpMeta::read_from_archive(&archive.path).unwrap_err();
    assert_eq!(error.code, ErrorCode::ArchiveCorrupt);
    assert!(error.message.contains(DUMP_META_MEMBER));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn read_from_archive_refuses_a_missing_archive() {
    let error = DumpMeta::read_from_archive(Path::new(
        "/nonexistent/clio/no-such-archive.cliobak.tar.gz",
    ))
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound, "{error}");
}

/// A SQLite archive that carries a key member is three members: manifest,
/// payload, and key store. It still carries no dump meta, so that is the answer
/// the operator gets.
///
/// A member cap below three would answer a member-count refusal here instead,
/// which says nothing about the archive being the wrong kind of snapshot.
#[test]
fn read_from_archive_names_the_dump_meta_on_an_archive_with_a_key_member() {
    let dir = std::env::temp_dir().join(format!("clio-dumpmeta-keys-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let payload = crate::staging::stage_bytes(b"{}", &dir, "test").unwrap();
    let keys = crate::fixtures::key_source(&dir, 2);
    let request = crate::CreateRequest {
        backup_dir: dir.clone(),
        slug: "clio".to_owned(),
        backend: "sqlite".to_owned(),
        database: "clio.db".to_owned(),
        store_schema: None,
        schema_version: None,
        payload,
        payload_member: "store.db".to_owned(),
        method: "vacuum-into".to_owned(),
        consistency: "point-in-time".to_owned(),
        started_at: "2026-01-01T00:00:00Z".to_owned(),
        duration_ms: 1,
        counts_complete: true,
        additional_members: Vec::new(),
        keys: Some(keys),
        label: None,
        note: None,
        tool_version: "test".to_owned(),
        authority: None,
    };
    let archive = crate::create_archive(&request).unwrap();
    // The archive really does carry the three members the cap has to allow.
    let names = {
        let file = std::fs::File::open(&archive.path).unwrap();
        let mut reader = crate::container::ArchiveReader::new(file).unwrap();
        let mut names = Vec::new();
        while let Some(header) = reader.next_member().unwrap() {
            names.push(header.name);
            reader.skip_current().unwrap();
        }
        names
    };
    assert_eq!(
        names.len(),
        3,
        "manifest, payload, and key member: {names:?}"
    );

    let error = DumpMeta::read_from_archive(&archive.path).unwrap_err();
    assert_eq!(error.code, ErrorCode::ArchiveCorrupt, "{error}");
    assert!(error.message.contains(DUMP_META_MEMBER), "{error}");
    assert!(
        !error.message.contains("member count"),
        "the refusal is about the missing member, not a cap: {error}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn validate_dump_meta_file_refuses_a_missing_file_and_a_directory() {
    let error =
        validate_dump_meta_file(Path::new("/nonexistent/clio/no-such-dump-meta.json")).unwrap_err();
    assert_eq!(error.code, ErrorCode::Internal, "{error}");
    // A directory opens but cannot be read, so the read failure is its own
    // arm; the staged meta is always a file, and a directory there is a
    // staging bug this check must refuse loudly.
    let dir = std::env::temp_dir().join(format!("clio-dumpmeta-dir-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let error = validate_dump_meta_file(&dir).unwrap_err();
    assert_eq!(error.code, ErrorCode::Internal, "{error}");
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn validate_dump_meta_file_refuses_an_over_bound_file() {
    // The read is bounded by take(MAX + 1), so a file at the bound is fine and
    // one byte over is refused; this constructs the refusal without staging a
    // real oversized member.
    let dir = std::env::temp_dir().join(format!("clio-dumpmeta-big-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("big-meta.json");
    let body = vec![b' '; MAX_DUMP_META_BYTES as usize + 1];
    std::fs::write(&path, &body).unwrap();
    let error = validate_dump_meta_file(&path).unwrap_err();
    assert_eq!(error.code, ErrorCode::ArchiveCorrupt, "{error}");
    let _ = std::fs::remove_dir_all(&dir);
}
