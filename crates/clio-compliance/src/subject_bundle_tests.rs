// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the subject-bundle tests on the SQLite
//! fixture: the carried planned families and pinned counts, the checksum
//! over the bytes, the balanced family's positive path with the bundle
//! staying open, the named omissions for the unknown, shared, and capped
//! families, the restart equality without a written destination, the
//! changed-fixture consistency, and the empty selection.
//!
//! ## Owns
//! - Every assertion about `build_subject_bundle` output on SQLite
//!
//! ## Does not own
//! - The bundle (`subject_bundle.rs`) and the family selection
//!   (`inventory_map.rs`)
//! - The fixture store (`inventory_map_fixtures.rs`) and the class writers
//!   (`inventory_fixture.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the SQLite subject-bundle tests.

use super::{
    FamilyClass, SubjectBundle, SubjectBundleManifest, SubjectFamilyInput, build_subject_bundle,
};
use crate::bundle::sha256_hex_bytes;
use crate::export::HISTORY_LIMIT;
use crate::inventory::{InventoryFacts, InventoryInput, scope_inventory};
use crate::inventory_fixture::{ACTOR, OTHER, SELECTED, TS, carrier, fixture_id};
use crate::inventory_map_fixtures::{FIXTURE_COUNTS, MapFixture};
use crate::test_unique::unique;
use clio_store::{HistoryStore, Store};
use clio_types::{EpisodicType, TaskRecord};
use serde_json::Value;
use std::path::Path;

/// One bundle for `subject_id` over the fixture's live store file.
fn build(fixture: &MapFixture, subject_id: &str) -> SubjectBundle {
    let facts = fixture.facts();
    let inventory = scope_inventory(&InventoryInput {
        facts: facts.clone(),
        paths: fixture.paths(),
        declared_copies: Vec::new(),
        peers_enabled: Some(false),
    });
    build_subject_bundle(&SubjectFamilyInput {
        store: &fixture.store,
        bank_id: &fixture.bank,
        subject_id,
        facts: &facts,
        inventory: &inventory,
    })
    .expect("build the subject bundle")
}

/// The selection bytes parsed as JSON, or a panic.
fn parsed(bundle: &SubjectBundle) -> Value {
    serde_json::from_slice(&bundle.bytes).expect("parse the selection bytes")
}

/// One family's manifest selection, or a panic when the manifest has none.
fn family_manifest<'a>(bundle: &'a SubjectBundle, name: &str) -> &'a super::FamilySelection {
    bundle
        .manifest
        .families
        .iter()
        .find(|selection| selection.family == name)
        .unwrap_or_else(|| panic!("the manifest has no {name} family"))
}

/// Assert no selected id and no omission text of `bundle` names another
/// subject, in the manifest or in the bytes.
fn assert_no_other_subject(bundle: &SubjectBundle) {
    for selection in &bundle.manifest.families {
        assert!(
            selection.selected_ids.iter().all(|id| !id.contains(OTHER)),
            "{} selected another subject's record: {:?}",
            selection.family,
            selection.selected_ids
        );
        assert!(
            selection
                .omission
                .as_deref()
                .is_none_or(|reason| !reason.contains(OTHER)),
            "{} names another subject: {:?}",
            selection.family,
            selection.omission
        );
    }
    assert!(
        bundle
            .manifest
            .omissions
            .iter()
            .all(|reason| !reason.contains(OTHER)),
        "the omission list names another subject: {:?}",
        bundle.manifest.omissions
    );
    let text = String::from_utf8(bundle.bytes.clone()).expect("the bytes are UTF-8");
    assert!(!text.contains(OTHER), "the bytes name another subject");
}

/// The sorted relative file names under `root`, for the no-destination check.
fn dir_names(root: &Path) -> Vec<String> {
    fn collect(root: &Path, dir: &Path, names: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            names.push(
                path.strip_prefix(root)
                    .expect("entry lives under the root")
                    .display()
                    .to_string(),
            );
            if path.is_dir() {
                collect(root, &path, names);
            }
        }
    }
    let mut names = Vec::new();
    collect(root, root, &mut names);
    names.sort();
    names
}

/// The two fixture directories a destination write would have to touch.
fn watched_dirs(fixture: &MapFixture) -> Vec<std::path::PathBuf> {
    vec![
        fixture
            .paths()
            .db_path
            .parent()
            .expect("the store dir")
            .to_path_buf(),
        fixture.paths().backup_dir,
    ]
}

#[test]
fn every_planned_family_is_carried_in_the_bytes_and_manifest() {
    let fixture = MapFixture::create();
    let bundle = build(&fixture, SELECTED);

    let value = parsed(&bundle);
    assert_eq!(
        value.get("format").and_then(Value::as_str),
        Some("clio.subject_selection")
    );
    assert_eq!(value.get("format_version").and_then(Value::as_u64), Some(1));
    assert_eq!(
        value.get("subject_id").and_then(Value::as_str),
        Some(SELECTED)
    );
    assert_eq!(
        value.get("bank_id").and_then(Value::as_str),
        Some(fixture.bank.as_str())
    );
    assert_eq!(value.get("backend").and_then(Value::as_str), Some("sqlite"));
    let families = value
        .get("families")
        .and_then(Value::as_array)
        .expect("the bytes carry one selection per planned family");
    assert_eq!(families.len(), bundle.manifest.families.len());

    // The manifest's exact family counts are the pinned fixture counts.
    let observed: Vec<(&str, u64, u64, bool)> = bundle
        .manifest
        .families
        .iter()
        .map(|s| (s.family, s.selected, s.omitted, s.omission.is_some()))
        .collect();
    assert_eq!(observed, FIXTURE_COUNTS, "the pinned per-family counts");
    assert_eq!(bundle.manifest.selected_total, 10);
    assert_eq!(bundle.manifest.omitted_total, 66);
    assert!(!bundle.manifest.complete, "named omissions keep it open");
    assert_eq!(
        bundle.manifest.content_sha256,
        sha256_hex_bytes(&bundle.bytes),
        "the manifest checksum describes the carried bytes"
    );
    assert_eq!(bundle.manifest.content_bytes, bundle.bytes.len() as u64);
    bundle
        .verify_checksum()
        .expect("a fresh bundle verifies its own checksum");
    assert_no_other_subject(&bundle);
}

#[test]
fn a_changed_byte_fails_the_checksum() {
    let fixture = MapFixture::create();
    let bundle = build(&fixture, SELECTED);

    let mut changed = bundle.bytes.clone();
    let last = changed.len() - 1;
    changed[last] ^= 0x01;
    let mutated = SubjectBundle {
        bytes: changed,
        manifest: clone_manifest(&bundle),
    };
    let err = mutated
        .verify_checksum()
        .expect_err("a changed byte must fail the checksum");
    assert!(
        err.to_string().contains("checksum mismatch"),
        "the failure names the checksum: {err}"
    );
    // The untouched bundle still verifies.
    bundle
        .verify_checksum()
        .expect("the original still verifies");
}

/// A fresh manifest copy of `bundle`'s, for the mutated-bytes construction.
fn clone_manifest(bundle: &SubjectBundle) -> SubjectBundleManifest {
    let m = &bundle.manifest;
    super::SubjectBundleManifest {
        format_version: m.format_version,
        backend: m.backend,
        bank_id: m.bank_id.clone(),
        subject_id: m.subject_id.clone(),
        content_sha256: m.content_sha256.clone(),
        content_bytes: m.content_bytes,
        selected_total: m.selected_total,
        omitted_total: m.omitted_total,
        complete: m.complete,
        omissions: m.omissions.clone(),
        families: m.families.clone(),
    }
}

#[test]
fn unknown_and_shared_families_name_omissions_and_stay_incomplete() {
    let fixture = MapFixture::create();
    let bundle = build(&fixture, SELECTED);

    // Unknown link: the hashed tombstone family selects nothing.
    let tombstones = family_manifest(&bundle, "tombstones");
    assert_eq!(tombstones.class, FamilyClass::UnknownLink);
    assert_eq!(tombstones.selected, 0);
    let reason = tombstones.omission.as_deref().expect("tombstones name it");
    assert!(reason.contains("one-way subject_hash"), "{reason}");

    // Shared: bank-scoped rows can never be one subject's.
    for name in ["assoc_edges", "persona_stable", "persona_preferences"] {
        let selection = family_manifest(&bundle, name);
        assert_eq!(selection.class, FamilyClass::InseparableShared);
        assert_eq!(selection.selected, 0);
        assert!(!selection.complete, "{name} stays incomplete");
    }

    assert!(!bundle.manifest.complete);
    assert_no_other_subject(&bundle);
}

#[test]
fn a_capped_history_read_names_the_cap_and_stays_incomplete() {
    let fixture = MapFixture::create();
    // Fill the task history to its cap so the bank-wide read is truncated.
    for index in 0..HISTORY_LIMIT {
        let item_id = format!("{}-cap-item-{index}", fixture_id(SELECTED, "cap"));
        let item = carrier(&item_id, &fixture.bank, SELECTED, EpisodicType::Task);
        let task = TaskRecord {
            context: None,
            id: format!("{}-cap-task-{index}", fixture_id(SELECTED, "cap")),
            bank_id: fixture.bank.clone(),
            item_id: item_id.clone(),
            task_id: format!("{}-cap-{index}", fixture_id(SELECTED, "cap")),
            subject_id: SELECTED.into(),
            version: 1,
            definition: "fill the cap".into(),
            steps: vec!["write".into()],
            status: "in_progress".into(),
            evidence_ref: None,
            admission_score: 0.5,
            created_at: TS.into(),
        };
        fixture
            .store
            .commit_task_upsert(&item, &task, ACTOR)
            .expect("write one capped-fixture task");
    }

    let bundle = build(&fixture, SELECTED);
    let tasks = family_manifest(&bundle, "tasks");
    let reason = tasks.omission.as_deref().expect("the cap is named");
    assert!(
        reason.contains(&format!("capped at {HISTORY_LIMIT} rows")),
        "{reason}"
    );
    assert!(!tasks.complete && !bundle.manifest.complete);
    assert_no_other_subject(&bundle);
}

#[test]
fn the_same_subject_builds_identical_bytes_after_a_store_restart() {
    let mut fixture = MapFixture::create();
    let before = build(&fixture, SELECTED);
    let watched = watched_dirs(&fixture);
    let names_before: Vec<Vec<String>> = watched.iter().map(|dir| dir_names(dir)).collect();

    fixture.restart();
    let after = build(&fixture, SELECTED);

    assert_eq!(
        before.bytes, after.bytes,
        "the same subject builds identical bytes after a restart"
    );
    assert_eq!(
        before.manifest.content_sha256,
        after.manifest.content_sha256
    );
    assert_eq!(before.manifest.families, after.manifest.families);
    assert_eq!(before.manifest.complete, after.manifest.complete);
    after
        .verify_checksum()
        .expect("the paired manifest verifies");
    // No destination was written: the watched directories hold the same names.
    let names_after: Vec<Vec<String>> = watched.iter().map(|dir| dir_names(dir)).collect();
    assert_eq!(names_before, names_after, "no destination file appeared");
}

#[test]
fn a_fixture_change_during_construction_keeps_counts_and_checksum_on_the_same_bytes() {
    let fixture = MapFixture::create();
    let facts = fixture.facts();
    // The store changes after the facts snapshot, during "construction".
    let removed = fixture
        .store
        .delete_item(&fixture.bank, &fixture_id(SELECTED, "item"))
        .expect("remove one stored item row");
    assert!(removed, "the fixture item must exist before the removal");

    let inventory = scope_inventory(&InventoryInput {
        facts: facts.clone(),
        paths: fixture.paths(),
        declared_copies: Vec::new(),
        peers_enabled: Some(false),
    });
    let bundle = build_subject_bundle(&SubjectFamilyInput {
        store: &fixture.store,
        bank_id: &fixture.bank,
        subject_id: SELECTED,
        facts: &facts,
        inventory: &inventory,
    })
    .expect("build over the changed store");

    // Counts and checksum describe the same bytes: one mapping run.
    let items = family_manifest(&bundle, "items");
    let reason = items.omission.as_deref().expect("the delta is named");
    assert!(reason.contains("store-wide"), "{reason}");
    let value = parsed(&bundle);
    let byte_items = value
        .get("families")
        .and_then(Value::as_array)
        .and_then(|rows| {
            rows.iter()
                .find(|row| row.get("family").and_then(Value::as_str) == Some("items"))
        })
        .expect("the bytes carry the items family");
    assert_eq!(
        byte_items.get("selected").and_then(Value::as_u64),
        Some(items.selected),
        "the bytes and the manifest carry the same items count"
    );
    assert_eq!(
        byte_items.get("omitted").and_then(Value::as_u64),
        Some(items.omitted)
    );
    bundle
        .verify_checksum()
        .expect("counts and checksum describe the same bytes");
    assert!(!bundle.manifest.complete);
    assert_no_other_subject(&bundle);
}

#[test]
fn a_subject_with_no_records_builds_an_empty_named_incomplete_selection() {
    let fixture = MapFixture::create();
    let absent = unique("inv-bundle-absent");
    let bundle = build(&fixture, &absent);

    let items = family_manifest(&bundle, "items");
    assert_eq!(items.selected, 0);
    assert_eq!(items.class, FamilyClass::Selected);
    let reason = items.omission.as_deref().expect("the empty pick is named");
    assert!(reason.contains("store-wide"), "{reason}");
    assert_eq!(bundle.manifest.selected_total, 0);
    assert!(bundle.manifest.omitted_total > 0);
    assert!(!bundle.manifest.complete);
    bundle.verify_checksum().expect("the bytes still verify");
    assert_no_other_subject(&bundle);
}

#[test]
fn a_balanced_family_is_complete_while_the_bundle_stays_open() {
    let fixture = MapFixture::create();
    // The positive path: measured task rows all bound, so the family names
    // no omission and is complete. The same balanced-facts shape the mapping
    // tests use.
    let mut facts = fixture.facts();
    let InventoryFacts::Sqlite(state) = &mut facts else {
        panic!("the fixture is a SQLite store");
    };
    for (name, count) in &mut state.table_counts {
        if name == "task_records" {
            *count = 1;
        }
    }
    let inventory = scope_inventory(&InventoryInput {
        facts: facts.clone(),
        paths: fixture.paths(),
        declared_copies: Vec::new(),
        peers_enabled: Some(false),
    });
    let bundle = build_subject_bundle(&SubjectFamilyInput {
        store: &fixture.store,
        bank_id: &fixture.bank,
        subject_id: SELECTED,
        facts: &facts,
        inventory: &inventory,
    })
    .expect("build the bundle over the balanced facts");

    let tasks = family_manifest(&bundle, "tasks");
    assert_eq!((tasks.selected, tasks.omitted), (1, 0));
    assert!(tasks.omission.is_none() && tasks.complete);
    // The fixed plan always includes families with no per-subject read, so
    // the bundle-level flag stays false even on the positive path.
    assert!(!bundle.manifest.complete);
    bundle.verify_checksum().expect("the bytes still verify");
    assert_no_other_subject(&bundle);
}
