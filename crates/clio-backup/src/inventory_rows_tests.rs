// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the backup listing: manifest-time
//! ordering with a deliberately wrong mtime, the limit, the empty directory,
//! a corrupt archive listed in place with its reason, and the JSON row shape.
//!
//! ## Owns
//! - The listing assertions over archives produced by `create_archive`
//!
//! ## Does not own
//! - Create and verify cases (see their own test modules)
//! - The container reader (see `container_corrupt_tests`)
//!
//! ## Boundary
//!
//! Keep this file focused on the listing behavior of `inventory`.

use super::*;
use crate::fixtures::{Scratch, request, staged_payload};
use crate::{KEY_MEMBER, create::create_archive};
use std::path::Path;

/// One archive, with its manifest time rewritten so the listing order is
/// testable without waiting for the clock.
fn archive_at(scratch: &Scratch, seed: u8, created_at: &str, label: &str) -> PathBuf {
    let payload = staged_payload(scratch.path(), seed);
    let mut req = request(scratch.path(), payload);
    req.label = Some(label.to_owned());
    let report = create_archive(&req).expect("create");
    rewrite_created_at(&report.path, created_at);
    report.path
}

/// Rebuild the archive with a different manifest `created_at`.
fn rewrite_created_at(path: &Path, created_at: &str) {
    use std::io::Write;
    let file = std::fs::File::open(path).expect("open");
    let mut reader = crate::container::ArchiveReader::new(file).expect("read");
    let mut manifest = Vec::new();
    assert!(reader.next_member().unwrap().is_some());
    reader.read_current(&mut manifest).unwrap();
    let mut payload = Vec::new();
    assert!(reader.next_member().unwrap().is_some());
    reader.read_current(&mut payload).unwrap();
    let text = String::from_utf8(manifest).expect("utf-8");
    let start = text.find("\"created_at\":\"").expect("created_at field") + 14;
    let end = start + 20;
    let mut rewritten = text[..start].to_owned();
    rewritten.push_str(created_at);
    rewritten.push_str(&text[end..]);
    let rewritten = rewritten.into_bytes();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(
            &crate::tar_header::build_header(crate::MANIFEST_MEMBER, rewritten.len() as u64)
                .unwrap(),
        )
        .unwrap();
    encoder.write_all(&rewritten).unwrap();
    encoder
        .write_all(&vec![0u8; (512 - rewritten.len() % 512) % 512])
        .unwrap();
    encoder
        .write_all(&crate::tar_header::build_header("store.db", payload.len() as u64).unwrap())
        .unwrap();
    encoder.write_all(&payload).unwrap();
    encoder
        .write_all(&vec![0u8; (512 - payload.len() % 512) % 512])
        .unwrap();
    encoder.write_all(&[0u8; 1024]).unwrap();
    std::fs::write(path, encoder.finish().unwrap()).expect("write");
}

#[test]
fn an_empty_directory_lists_nothing_and_is_not_an_error() {
    let scratch = Scratch::new("list-empty");

    let rows = list_snapshot_rows(scratch.path(), None).expect("an empty directory lists");

    assert!(rows.is_empty(), "{rows:?}");
    assert_eq!(latest_snapshot_id(scratch.path()).expect("latest"), None);
}

#[test]
fn a_missing_directory_is_refused_by_name() {
    let scratch = Scratch::new("list-absent");

    let error = list_snapshot_rows(&scratch.join("nope"), None).expect_err("no directory");

    assert_eq!(error.code, clio_types::ErrorCode::NotFound);
}

#[test]
fn one_archive_lists_with_its_manifest_facts() {
    let scratch = Scratch::new("list-one");
    let payload = staged_payload(scratch.path(), 30);
    let mut req = request(scratch.path(), payload);
    req.keys = Some(crate::fixtures::key_source(scratch.path(), 3));
    let report = create_archive(&req).expect("create");

    let rows = list_snapshot_rows(scratch.path(), None).expect("list");

    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.id, report.id);
    assert_eq!(row.backend, "sqlite");
    assert_eq!(row.slug, "clio");
    assert_eq!(row.payload_bytes, req.payload.bytes());
    assert_eq!(row.archive_bytes, report.archive_bytes);
    assert!(row.keys_included);
    assert!(row.unreadable.is_none());
}

#[test]
fn many_archives_list_newest_first_by_manifest_time_not_mtime() {
    let scratch = Scratch::new("list-order");
    let oldest = archive_at(&scratch, 31, "2026-01-01T00:00:00Z", "oldest");
    let newest = archive_at(&scratch, 32, "2026-03-01T00:00:00Z", "newest");
    let middle = archive_at(&scratch, 33, "2026-02-01T00:00:00Z", "middle");

    // Make the file times say the opposite of the manifest times.
    set_mtime(&oldest, 2_000_000_000);
    set_mtime(&newest, 1_000_000_000);
    set_mtime(&middle, 1_500_000_000);

    let rows = list_snapshot_rows(scratch.path(), None).expect("list");
    let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
    let id_of = |path: &Path| crate::archive_name::snapshot_id_of(path).expect("a snapshot id");

    assert_eq!(
        ids,
        vec![
            id_of(&newest).as_str(),
            id_of(&middle).as_str(),
            id_of(&oldest).as_str()
        ],
        "the manifest time decides, not the file time"
    );
    assert_eq!(rows[0].label.as_deref(), Some("newest"));
}

#[test]
fn a_limit_keeps_the_newest_rows() {
    let scratch = Scratch::new("list-limit");
    archive_at(&scratch, 34, "2026-01-01T00:00:00Z", "oldest");
    archive_at(&scratch, 35, "2026-03-01T00:00:00Z", "newest");
    archive_at(&scratch, 36, "2026-02-01T00:00:00Z", "middle");

    let rows = list_snapshot_rows(scratch.path(), Some(2)).expect("list");

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].label.as_deref(), Some("newest"));
    assert_eq!(rows[1].label.as_deref(), Some("middle"));
    assert_eq!(
        list_snapshot_rows(scratch.path(), Some(0)).unwrap().len(),
        0
    );
}

#[test]
fn the_latest_id_is_the_newest_manifest_time() {
    let scratch = Scratch::new("list-latest");
    archive_at(&scratch, 37, "2026-01-01T00:00:00Z", "oldest");
    let newest = archive_at(&scratch, 38, "2026-05-05T00:00:00Z", "newest");

    let id = latest_snapshot_id(scratch.path()).expect("latest");

    assert_eq!(
        id,
        Some(crate::archive_name::snapshot_id_of(&newest).expect("a snapshot id"))
    );
}

#[test]
fn a_corrupt_archive_lists_in_place_with_its_reason() {
    let scratch = Scratch::new("list-corrupt");
    archive_at(&scratch, 39, "2026-04-01T00:00:00Z", "good");
    let broken = scratch.join("broken.cliobak.tar.gz");
    std::fs::write(&broken, b"not an archive at all").expect("write a broken archive");

    let rows = list_snapshot_rows(scratch.path(), None).expect("list");

    assert_eq!(rows.len(), 2, "one bad file never hides the rest");
    let bad = rows
        .iter()
        .find(|row| row.unreadable.is_some())
        .expect("the broken archive is listed");
    assert_eq!(bad.id, "broken");
    assert_eq!(bad.backend, "unreadable");
    let unreadable = bad.unreadable.as_deref().unwrap_or_default();
    assert!(!unreadable.is_empty(), "{unreadable:?}");
    // A readable archive still carries its facts.
    assert!(
        rows.iter()
            .any(|row| row.label.as_deref() == Some("good") && row.unreadable.is_none())
    );
}

#[test]
fn an_unreadable_archive_sorts_after_the_readable_ones() {
    let scratch = Scratch::new("list-corrupt-order");
    let good = archive_at(&scratch, 40, "2020-01-01T00:00:00Z", "good");
    let broken = scratch.join("aaa-broken.cliobak.tar.gz");
    std::fs::write(&broken, b"not an archive").expect("write a broken archive");

    let rows = list_snapshot_rows(scratch.path(), None).expect("list");

    assert_eq!(rows.len(), 2);
    assert!(
        rows[0].unreadable.is_none(),
        "a readable snapshot is never pushed off the top by a broken file"
    );
    assert!(rows[1].unreadable.is_some());
    assert_ne!(
        rows[0].id,
        crate::archive_name::snapshot_id_of(&broken).expect("an id")
    );
    assert!(good.exists());
}

#[test]
fn a_row_renders_stable_json_fields() {
    let scratch = Scratch::new("list-json");
    archive_at(&scratch, 41, "2026-06-01T00:00:00Z", "labelled");

    let rows = list_snapshot_rows(scratch.path(), None).expect("list");

    // The derived `Serialize` fixes the field order a script reads.
    let rendered = serde_json::to_string(&rows).expect("render");
    assert!(rendered.starts_with(r#"[{"id":"#), "{rendered}");
    for field in [
        "\"id\"",
        "\"created_at\"",
        "\"backend\"",
        "\"slug\"",
        "\"archive_bytes\"",
        "\"payload_bytes\"",
        "\"keys_included\"",
        "\"label\"",
        "\"unreadable\"",
    ] {
        assert!(rendered.contains(field), "{field} missing from {rendered}");
    }
    let position = |field: &str| rendered.find(field).expect("field present");
    assert!(position("\"id\"") < position("\"created_at\""));
    assert!(position("\"created_at\"") < position("\"backend\""));
    assert!(position("\"backend\"") < position("\"slug\""));
    assert!(position("\"slug\"") < position("\"archive_bytes\""));
    assert!(position("\"archive_bytes\"") < position("\"payload_bytes\""));
    assert!(position("\"payload_bytes\"") < position("\"keys_included\""));
    assert!(position("\"keys_included\"") < position("\"label\""));
    assert!(position("\"label\"") < position("\"unreadable\""));
    let json: serde_json::Value = serde_json::from_str(&rendered).expect("parse");
    let row = &json.as_array().expect("an array")[0];
    assert_eq!(row["label"], serde_json::json!("labelled"));
    assert_eq!(row["unreadable"], serde_json::Value::Null);
    assert!(row["payload_bytes"].as_u64().unwrap_or(0) > 0);
}

#[test]
fn the_listing_reads_no_payload_and_no_database() {
    let scratch = Scratch::new("list-cheap");
    // A large member makes an accidental full inflate expensive in time and
    // impossible to miss in a profile; the listing must not touch it.
    let source = scratch.join("big.bin");
    std::fs::write(&source, vec![7u8; 4 * 1024 * 1024]).expect("write a large payload");
    let payload = crate::staging::stage_copy(&source, scratch.path(), "payload").expect("stage");
    let report = create_archive(&request(scratch.path(), payload)).expect("create");
    assert!(report.archive_bytes < 100_000, "the archive is compressed");

    let rows = list_snapshot_rows(scratch.path(), None).expect("list");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].payload_bytes, 4 * 1024 * 1024);
    // A payload read would have left a staged file; none exists.
    let staged: Vec<String> = std::fs::read_dir(scratch.path())
        .expect("read")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("verify-payload"))
        .collect();
    assert!(staged.is_empty(), "{staged:?}");
    assert!(!KEY_MEMBER.is_empty(), "{KEY_MEMBER:?}");
}

/// Set a file's modification time to `secs` since the epoch.
fn set_mtime(path: &Path, secs: u64) {
    let file = std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open to touch");
    file.set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs))
        .expect("set the modification time");
}
