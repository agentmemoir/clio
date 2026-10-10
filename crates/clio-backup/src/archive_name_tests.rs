// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing snapshot naming: the id and file
//! name shapes, the collision suffix, both UTC forms, the id a file name
//! carries, and the slug gate.
//!
//! ## Owns
//! - The naming and slug-gate assertions
//!
//! ## Does not own
//! - Create, verify, or inventory cases (see their own test modules)
//!
//! ## Boundary
//!
//! Keep this file focused on `archive_name` behavior.

use super::*;
use crate::SNAPSHOT_EXTENSION;

#[test]
fn an_id_is_a_slug_a_compact_stamp_and_eight_hex_characters() {
    let id = snapshot_id("clio", 1_789_000_000, "0a1b2c3d");

    assert_eq!(id, "clio-20260910T002640Z-0a1b2c3d");
    assert_eq!(archive_file_name(&id), format!("{id}{SNAPSHOT_EXTENSION}"));
    assert!(id.ends_with("-0a1b2c3d"));
    assert!(!id.contains('/') && !id.contains('\\'));
}

#[test]
fn the_compact_and_rfc3339_forms_describe_the_same_instant() {
    let seconds = 1_789_000_000;
    let compact = compact_utc(seconds);
    let rfc3339 = rfc3339_utc(seconds);

    assert_eq!(compact, "20260910T002640Z");
    assert_eq!(rfc3339, "2026-09-10T00:26:40Z");
    assert_eq!(compact.len(), 16);
    assert_eq!(&rfc3339[0..4], &compact[0..4]);
    assert_eq!(&rfc3339[5..7], &compact[4..6]);
    assert_eq!(&rfc3339[8..10], &compact[6..8]);
    assert_eq!(&rfc3339[11..13], &compact[9..11]);
    assert_eq!(&rfc3339[14..16], &compact[11..13]);
    assert_eq!(&rfc3339[17..19], &compact[13..15]);
}

#[test]
fn the_epoch_and_a_leap_day_both_format() {
    assert_eq!(compact_utc(0), "19700101T000000Z");
    assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
    assert_eq!(rfc3339_utc(1_709_210_096), "2024-02-29T12:34:56Z");
    // 2000-03-01: the day after a leap day inside a century leap year.
    assert_eq!(rfc3339_utc(951_868_800), "2000-03-01T00:00:00Z");
}

#[test]
fn the_collision_suffix_is_eight_hex_characters_and_moves() {
    let first = collision_suffix();
    let second = collision_suffix();

    assert_eq!(first.len(), 8);
    assert_eq!(second.len(), 8);
    assert_ne!(first, second, "two creates never share a suffix");
    assert!(
        first
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
        "the suffix is lowercase hex: {first}"
    );
}

#[test]
fn a_file_name_carries_its_id_and_other_files_carry_none() {
    let id = snapshot_id("clio", 1_789_000_000, "0a1b2c3d");
    let name = archive_file_name(&id);

    assert_eq!(snapshot_id_of(std::path::Path::new(&name)), Some(id));
    assert_eq!(snapshot_id_of(std::path::Path::new("notes.txt")), None);
    assert_eq!(
        snapshot_id_of(std::path::Path::new(SNAPSHOT_EXTENSION)),
        None,
        "an empty stem is not a snapshot id"
    );
}

#[test]
fn a_safe_slug_passes_and_an_unsafe_one_names_why() {
    assert!(check_slug("clio").is_ok());
    assert!(check_slug("clio_2-test.db").is_ok());
    for (slug, why) in [
        ("", "empty"),
        ("a/b", "separator"),
        ("a\\b", "backslash"),
        ("a\nb", "control"),
        ("a b", "space is refused"),
        ("caf\u{e9}", "non-ascii"),
    ] {
        let error = check_slug(slug).expect_err("an unsafe slug is refused");
        assert!(
            error.message.contains("slug"),
            "the message names the slug: {}",
            error.message
        );
        let _ = why;
    }
    let long = "x".repeat(65);
    assert!(check_slug(&long).is_err(), "an over-long slug is refused");
}

#[test]
fn the_clock_never_goes_backwards_through_a_nanosecond() {
    let now = now_seconds();
    assert!(now > 1_700_000_000, "the host clock is set: {now}");
    assert!(now_seconds() >= now);
}
