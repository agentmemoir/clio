// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests `clio backup list`: the empty, one, and many cases, the
//! manifest-time ordering, the limit, a corrupt archive listed in place, the
//! store-down case, and the flag refusals.
//!
//! ## Owns
//! - The `list` behavior, view, and refusal assertions
//!
//! ## Does not own
//! - The command implementation (`backup_list_cli`)
//! - Archive creation (see `backup_create_cli_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::backup_cli_fixtures::{Scratch, populated_store, run_in, run_json_in, run_json_in_with};
use serde_json::json;

/// Take `count` snapshots into `scratch`'s backup directory.
fn snapshots(scratch: &Scratch, count: usize) {
    populated_store(&scratch.join("clio.db"), 2, 1);
    for _ in 0..count {
        let (code, payload, err) = run_json_in(scratch, &["create"]);
        assert_eq!(code, ExitCode::SUCCESS, "{err}");
        assert_eq!(payload["ok"], json!(true), "{err}");
    }
}

#[test]
fn an_empty_backup_directory_lists_nothing_and_exits_zero() {
    let scratch = Scratch::new("list-empty");
    std::fs::create_dir_all(scratch.join("backups")).expect("create the directory");

    let (code, payload, err) = run_json_in(&scratch, &["list"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["count"], json!(0), "{payload}");
    assert_eq!(payload["snapshots"].as_array().map(Vec::len), Some(0));
}

#[test]
fn a_missing_backup_directory_lists_nothing_and_exits_zero() {
    let scratch = Scratch::new("list-absent");

    let (code, payload, err) = run_json_in(&scratch, &["list"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["count"], json!(0), "{payload}");
}

#[test]
fn one_snapshot_lists_with_its_manifest_facts() {
    let scratch = Scratch::new("list-one");
    snapshots(&scratch, 1);

    let (code, payload, err) = run_json_in(&scratch, &["list"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["count"], json!(1), "{payload}");
    let row = &payload["snapshots"][0];
    assert_eq!(row["backend"], json!("sqlite"), "{row}");
    assert_eq!(row["slug"], json!("clio"), "{row}");
    assert_eq!(row["keys_included"], json!(true), "{row}");
    assert!(
        row["created_at"].as_str().unwrap_or("").ends_with('Z'),
        "{row}"
    );
    assert!(row["payload_bytes"].as_u64().unwrap_or(0) > 0, "{row}");
    assert!(row["archive_bytes"].as_u64().unwrap_or(0) > 0, "{row}");
    assert_eq!(row["unreadable"], json!(null), "{row}");
}

#[test]
fn many_snapshots_list_newest_first() {
    let scratch = Scratch::new("list-many");
    snapshots(&scratch, 3);

    let (code, payload, err) = run_json_in(&scratch, &["list"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["count"], json!(3), "{payload}");
    let times: Vec<&str> = payload["snapshots"]
        .as_array()
        .expect("an array")
        .iter()
        .map(|row| row["created_at"].as_str().expect("a time"))
        .collect();
    let mut sorted = times.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(times, sorted, "newest first by manifest time");
}

#[test]
fn a_limit_keeps_the_newest_rows() {
    let scratch = Scratch::new("list-limit");
    snapshots(&scratch, 3);

    let (code, payload, err) = run_json_in(&scratch, &["list", "--limit", "2"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["count"], json!(2), "{payload}");
    let (code, payload, err) = run_json_in(&scratch, &["list", "--limit", "0"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["count"], json!(0), "{payload}");
}

#[test]
fn a_corrupt_archive_lists_in_place_with_its_reason() {
    let scratch = Scratch::new("list-corrupt");
    snapshots(&scratch, 2);
    let broken = scratch.join("backups").join("broken.cliobak.tar.gz");
    std::fs::write(&broken, b"not an archive at all").expect("write a broken archive");

    let (code, payload, err) = run_json_in(&scratch, &["list"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(
        payload["count"],
        json!(3),
        "one bad file never hides the rest"
    );
    let bad = payload["snapshots"]
        .as_array()
        .expect("an array")
        .iter()
        .find(|row| row["id"] == json!("broken"))
        .expect("the broken archive is listed");
    assert_eq!(bad["backend"], json!("unreadable"), "{bad}");
    assert!(
        bad["unreadable"].as_str().unwrap_or("").len() > 5,
        "the reason is reported: {bad}"
    );
    let good = payload["snapshots"]
        .as_array()
        .expect("an array")
        .iter()
        .filter(|row| row["id"] != json!("broken"))
        .count();
    assert_eq!(good, 2, "the readable archives still list");
}

#[test]
fn a_corrupt_archive_is_named_in_the_text_view() {
    let scratch = Scratch::new("list-corrupt-text");
    snapshots(&scratch, 1);
    std::fs::write(
        scratch.join("backups").join("broken.cliobak.tar.gz"),
        b"not an archive",
    )
    .expect("write a broken archive");

    let (code, out, err) = run_in(&scratch, &["list"], true);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("broken unreadable"), "{out}");
}

#[test]
fn list_works_with_the_store_down() {
    let scratch = Scratch::new("list-store-down");
    snapshots(&scratch, 1);
    // The store is gone and the database URL points nowhere: a listing reads
    // archives, so neither matters.
    std::fs::remove_file(scratch.join("clio.db")).expect("remove the store");
    let (code, payload, err) = run_json_in_with(
        &scratch,
        &["list"],
        &[(
            "CLIO_DATABASE_URL",
            "postgres://clio:clio@127.0.0.1:9/nosuchdb",
        )],
    );

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["count"], json!(1), "{payload}");
}

#[test]
fn list_refuses_the_store_flags_it_cannot_honor() {
    let scratch = Scratch::new("list-refuse");

    let (code, _, err) = run_in(&scratch, &["list", "--db", "/tmp/whatever.db"], false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("--db"), "{err}");
    assert!(err.contains("cannot change the listing"), "{err}");

    let (code, _, err) = run_in(&scratch, &["list", "--backend", "postgres"], false);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("--backend"), "{err}");
}

#[test]
fn a_bad_limit_is_a_usage_error() {
    let scratch = Scratch::new("list-bad-limit");

    let (code, _, err) = run_in(&scratch, &["list", "--limit", "many"], false);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("count"), "{err}");
}

#[test]
fn list_help_documents_that_no_database_is_needed() {
    let scratch = Scratch::new("list-help");

    let (code, out, _) = run_in(&scratch, &["list", "--help"], false);

    assert_eq!(code, ExitCode::SUCCESS);
    assert!(out.contains("usage: clio backup list"), "{out}");
    assert!(out.contains("no payload, no database"), "{out}");
}

#[test]
fn the_text_view_names_every_snapshot() {
    let scratch = Scratch::new("list-text");
    snapshots(&scratch, 2);

    let (code, out, err) = run_in(&scratch, &["list"], true);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "{out}");
    for line in lines {
        assert!(line.contains("keys"), "{line}");
        assert!(line.contains("payload"), "{line}");
    }
}
