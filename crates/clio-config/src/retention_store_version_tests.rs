// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests proving a stored retention profile that
//! still carries a version 1 or 2 marker is converted to the current version on
//! load, with every value it holds preserved, and for the pins around that
//! conversion: a version 1 profile carrying a mission stays unconverted, and a
//! marker on a non-integer JSON lane keeps failing the whole store load.
//!
//! ## Owns
//! - The conversion of an older stored marker on the raw document before typing
//! - Value preservation and the equivalence of a converted profile to its
//!   current-version twin
//! - The write-back of the converted marker on the next save
//! - The v1 mission carve-out: a mission-carrying v1 profile is left unconverted
//!   and a mission-free one still converts
//! - The fail-closed outcome for a non-integer version marker
//!
//! ## Does not own
//! - The version gate on the request surface and its error (see `retention`)
//! - The fail-closed stored provenance floor (see `retention_store_floor_tests`)
//! - Bank resolution order and profile pruning (see `retention_store`)
//!
//! ## Boundary
//!
//! Changes to how an older stored marker is converted should normally require
//! changes here. Changes to profile fields or the version gate should not.
//!
//! Keep this module focused on the stored marker conversion.

use super::retention_store_tests::tmp;
use super::*;
use crate::retention::{DuplicateTolerance, RETENTION_PROFILE_VERSION, RecallScope, Verbosity};

/// A stored profile with a version 1 or 2 marker loads at the current version,
/// and every value it carries, in every profile of the file, is preserved.
#[test]
fn an_older_stored_marker_is_converted_to_the_current_version() {
    for version in [1_u32, 2] {
        let path = tmp(&format!("marker-{version}.json"));
        fs::write(
            &path,
            format!(
                r#"{{
                    "deployment": {{"verbosity": "selective", "version": {version}}},
                    "banks": {{
                        "bank-a": {{
                            "verbosity": "permissive",
                            "theta_offsets": {{"persona": 0.1}},
                            "duplicate_tolerance_write": "strict",
                            "duplicate_tolerance_read": "lenient",
                            "recall_scope_default": "consolidated_only",
                            "version": {version}
                        }}
                    }}
                }}"#
            ),
        )
        .expect("write");
        let store = RetentionStore::load(&path).expect("older store loads");

        let deployment = store.deployment.as_ref().expect("deployment");
        assert_eq!(
            deployment.version, RETENTION_PROFILE_VERSION,
            "a stored v{version} deployment marker converts"
        );
        assert_eq!(deployment.verbosity, Verbosity::Selective);

        let bank = store.bank_override("bank-a").expect("bank");
        assert_eq!(
            bank.version, RETENTION_PROFILE_VERSION,
            "a stored v{version} bank marker converts"
        );
        assert_eq!(bank.verbosity, Verbosity::Permissive);
        assert_eq!(bank.duplicate_tolerance_write, DuplicateTolerance::Strict);
        assert_eq!(bank.duplicate_tolerance_read, DuplicateTolerance::Lenient);
        assert_eq!(bank.recall_scope_default, RecallScope::ConsolidatedOnly);
        assert!((bank.theta_offsets["persona"] - 0.1).abs() < 1e-12);
        assert!(bank.mission.is_none(), "no mission is invented");

        let _ = fs::remove_file(&path);
    }
}

/// The conversion is verified by construction: a file with an older marker and
/// the same file with the current marker load to the identical store, so the
/// marker is the only thing the conversion changes.
#[test]
fn a_converted_store_matches_its_current_version_twin() {
    let earlier = tmp("twin-earlier.json");
    let current = tmp("twin-current.json");
    fs::write(&earlier, twin_fixture(2)).expect("write earlier");
    fs::write(&current, twin_fixture(RETENTION_PROFILE_VERSION)).expect("write current");

    let converted = RetentionStore::load(&earlier).expect("earlier loads");
    assert_eq!(
        converted,
        RetentionStore::load(&current).expect("current loads"),
        "a converted older file must equal its current-version twin"
    );
    let mission = converted
        .bank_override("bank-a")
        .expect("bank")
        .mission
        .as_ref()
        .expect("the stored mission survives");
    assert_eq!(mission.mission_statement, "keep durable outcomes");
    assert_eq!(mission.drop_examples.len(), 1);

    let _ = fs::remove_file(&earlier);
    let _ = fs::remove_file(&current);
}

/// The same fixture body with a different version marker.
fn twin_fixture(version: u32) -> String {
    format!(
        r#"{{
            "deployment": {{"verbosity": "selective", "version": {version}}},
            "banks": {{
                "bank-a": {{
                    "verbosity": "selective",
                    "duplicate_tolerance_write": "strict",
                    "mission": {{
                        "mission_statement": "keep durable outcomes",
                        "drop_examples": [{{"text": "heartbeat"}}]
                    }},
                    "version": {version}
                }}
            }}
        }}"#
    )
}

/// Loading converts the marker in memory; the file on disk is untouched until
/// the next save, which persists the converted marker.
#[test]
fn load_leaves_the_file_alone_and_save_writes_the_converted_marker_back() {
    let path = tmp("marker-write-back.json");
    fs::write(
        &path,
        r#"{"banks": {"bank-a": {"verbosity": "selective", "version": 1}}}"#,
    )
    .expect("write");
    let before = fs::read_to_string(&path).expect("read");
    assert!(
        before.contains(r#""version": 1"#),
        "the fixture carries the old marker"
    );

    let store = RetentionStore::load(&path).expect("load");
    assert_eq!(
        store.bank_override("bank-a").expect("bank").version,
        RETENTION_PROFILE_VERSION
    );
    assert_eq!(
        fs::read_to_string(&path).expect("read"),
        before,
        "loading the store must not rewrite the file"
    );

    store.save(&path).expect("save");
    assert!(
        fs::read_to_string(&path)
            .expect("read")
            .contains(&format!(r#""version": {RETENTION_PROFILE_VERSION}"#)),
        "the save persists the converted marker"
    );
    assert_eq!(RetentionStore::load(&path).expect("reload"), store);
    let _ = fs::remove_file(&path);
}

/// A version 1 profile that carries a mission is left unconverted: the marker is
/// not stamped current, so validation refuses the profile and the lossy read
/// prunes it — the mission is never activated.
#[test]
fn a_v1_profile_carrying_a_mission_is_left_unconverted() {
    let path = tmp("v1-mission.json");
    fs::write(
        &path,
        r#"{"banks": {"bank-a": {
            "verbosity": "selective",
            "mission": {
                "mission_statement": "keep durable outcomes",
                "drop_examples": [{"text": "heartbeat"}]
            },
            "version": 1
        }}}"#,
    )
    .expect("write");

    let store = RetentionStore::load(&path).expect("the typed read still succeeds");
    let bank = store.bank_override("bank-a").expect("bank");
    assert_eq!(bank.version, 1, "the v1 marker is not stamped current");
    assert!(
        bank.mission.is_some(),
        "the mission is read exactly as written"
    );

    let lossy = RetentionStore::load_lossy(&path);
    assert!(
        lossy.bank_override("bank-a").is_none(),
        "the profile is pruned, so its mission is never activated"
    );

    let _ = fs::remove_file(&path);
}

/// A version 1 profile without a mission still converts: the marker is re-stamped
/// to the current version exactly as before the mission carve-out.
#[test]
fn a_v1_profile_without_a_mission_is_still_converted() {
    let path = tmp("v1-no-mission.json");
    fs::write(
        &path,
        r#"{"banks": {"bank-a": {"verbosity": "selective", "version": 1}}}"#,
    )
    .expect("write");

    let store = RetentionStore::load(&path).expect("load");
    assert_eq!(
        store.bank_override("bank-a").expect("bank").version,
        RETENTION_PROFILE_VERSION,
        "a mission-free v1 profile is still re-stamped to the current version"
    );

    let _ = fs::remove_file(&path);
}

/// A version marker on a non-integer JSON lane is not converted, so the typed
/// read refuses the document: the whole store load fails closed, and the lossy
/// read degrades to the builtin store.
#[test]
fn a_non_integer_version_marker_fails_the_whole_store_load() {
    for marker in ["1.0", "\"1\""] {
        let path = tmp(&format!("marker-lane-{marker}.json"));
        fs::write(
            &path,
            format!(
                r#"{{"banks": {{"bank-a": {{"verbosity": "selective", "version": {marker}}}}}}}"#
            ),
        )
        .expect("write");

        let err = RetentionStore::load(&path).expect_err("the typed read refuses the lane");
        assert_eq!(err.code, ErrorCode::ConfigCorrupt, "lane {marker}");
        assert!(
            RetentionStore::load_lossy(&path)
                .bank_override("bank-a")
                .is_none(),
            "the lossy read degrades to the builtin store"
        );

        let _ = fs::remove_file(&path);
    }
}

/// Only versions 1 and 2 are converted. A current marker is already a no-op,
/// and a marker this build does not support is left for the existing refusal
/// path instead of being stamped current.
#[test]
fn only_version_one_and_two_markers_are_converted() {
    let path = tmp("marker-guard.json");
    fs::write(
        &path,
        r#"{"banks": {
            "current": {"version": 3},
            "future": {"version": 4},
            "zero": {"version": 0}
        }}"#,
    )
    .expect("write");

    let store = RetentionStore::load(&path).expect("load types the file");
    assert_eq!(
        store.bank_override("current").expect("current").version,
        RETENTION_PROFILE_VERSION
    );
    assert_eq!(
        store.bank_override("future").expect("future").version,
        4,
        "an unsupported marker is not stamped current"
    );
    assert_eq!(
        store.bank_override("zero").expect("zero").version,
        0,
        "version 0 was never a shipped marker and is not converted"
    );
    let pruned = RetentionStore::load_lossy(&path);
    assert!(pruned.bank_override("future").is_none());
    assert!(pruned.bank_override("zero").is_none());
    assert_eq!(
        pruned.bank_override("current"),
        store.bank_override("current")
    );

    let _ = fs::remove_file(&path);
}
