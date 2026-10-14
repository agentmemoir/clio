// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the live-Postgres twin of the subject-bundle
//! tests: the same pinned family counts as the SQLite slice, the checksum
//! verification and its changed-byte failure, the named unknown, shared, and
//! capped omissions, the leak-free selection, and the restart equality
//! without a written destination.
//!
//! ## Owns
//! - `PgBundleFixture`: the private-schema store and the KMS Arc a restart
//!   reopens under
//! - Every assertion about `build_subject_bundle` output on Postgres
//!
//! ## Does not own
//! - The bundle (`subject_bundle.rs`)
//! - The class writers and the shared-Postgres lock
//!   (`inventory_pg_support_tests.rs`, reused)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the live-Postgres subject-bundle tests.

use super::{
    FamilyClass, SubjectBundle, SubjectBundleManifest, SubjectFamilyInput, build_subject_bundle,
};
use crate::export::HISTORY_LIMIT;
use crate::inventory::{InventoryBackend, InventoryFacts};
use crate::inventory_fixture::{ACTOR, OTHER, SELECTED, TS, fixture_id};
use crate::inventory_pg_support_tests::{
    carrier, pg_checked_url, pg_skip_note, write_direct_link_class, write_hashed_link_class,
    write_shared_class,
};
use crate::test_unique::unique;
use clio_store::{
    DekProvider, HistoryStore, LocalDevKms, PgTestGuard, PostgresStore, ScopedSchema, lock_pg_test,
    postgres_target_state, probe_postgres,
};
use clio_types::{AmError, EpisodicType, ErrorCode, TaskRecord};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

/// The live fixture, or a printed credential-safe skip note plus `None`.
fn bundle_fixture_or_skip() -> Option<(PgTestGuard, PgBundleFixture)> {
    let pair = open_bundle_pg_fixture();
    if let Err(err) = &pair {
        eprintln!("{}", pg_skip_note("subject bundle Postgres tests", err));
    }
    pair.ok()
}

/// A private-schema store holding one row class per subject-link class, the
/// KMS Arc a restart reopens under, and the scoped schema.
pub(crate) struct PgBundleFixture {
    /// The store the fixture rows were written through; declared first so it
    /// drops, releasing its connections, before the schema below is dropped.
    pub(crate) store: PostgresStore,
    /// The key provider the store is sealed under; a restart reopens under it.
    pub(crate) kms: Arc<dyn DekProvider>,
    /// Bank holding the selected, other, and shared classes.
    pub(crate) bank: String,
    /// The private schema; dropped after the store.
    schema: ScopedSchema,
}

impl PgBundleFixture {
    /// Close the store and reopen the same scoped schema under the same KMS.
    pub(crate) fn restart(&mut self) {
        self.store = PostgresStore::open(self.schema.url(), Arc::clone(&self.kms))
            .expect("reopen the fixture store on the same scoped schema");
    }

    /// The live-store facts measured from the scoped target at call time.
    #[must_use]
    pub(crate) fn facts(&self) -> InventoryFacts {
        InventoryFacts::Postgres(
            postgres_target_state(self.schema.url()).expect("measure the scoped Postgres target"),
        )
    }
}

/// Open the live fixture: the shared-Postgres lock first, then a private
/// schema and a store holding one row class per subject-link class.
///
/// # Errors
/// The shared database is unreachable; the caller prints [`pg_skip_note`] and
/// returns. Fixture-write failures panic, because a reachable database that
/// cannot hold the fixture rows is a test failure, not a skip.
pub(crate) fn open_bundle_pg_fixture() -> Result<(PgTestGuard, PgBundleFixture), AmError> {
    let guard = lock_pg_test();
    let base = pg_checked_url();
    probe_postgres(&base)
        .map_err(|detail| AmError::new(ErrorCode::Internal, format!("postgres probe: {detail}")))?;
    let schema = ScopedSchema::new(&base);
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store =
        PostgresStore::open(schema.url(), Arc::clone(&kms)).expect("open the scoped fixture store");
    let bank = unique("inv-bundle-pg-bank");
    let erased_bank = unique("inv-bundle-pg-erased-bank");
    write_direct_link_class(&store, &bank, SELECTED);
    write_direct_link_class(&store, &bank, OTHER);
    write_shared_class(&store, &bank);
    crate::inventory_pg_tests::triples::write_triple_class(&store, &bank, &erased_bank);
    write_hashed_link_class(&store, &erased_bank);
    Ok((
        guard,
        PgBundleFixture {
            store,
            kms,
            bank,
            schema,
        },
    ))
}

/// One bundle for `subject_id` over the fixture's live scoped target.
fn build(fixture: &PgBundleFixture, subject_id: &str) -> SubjectBundle {
    let facts = fixture.facts();
    build_subject_bundle(&SubjectFamilyInput {
        store: &fixture.store,
        bank_id: &fixture.bank,
        subject_id,
        facts: &facts,
        backend: InventoryBackend::Postgres,
    })
    .expect("build the subject bundle")
}

/// Assert no selected id and no omission text of `bundle` names another
/// subject, in the manifest or in the bytes.
fn assert_no_other_subject(bundle: &SubjectBundle) {
    for selection in &bundle.manifest.families {
        assert!(
            selection.selected_ids.iter().all(|id| !id.contains(OTHER))
                && selection
                    .omission
                    .as_deref()
                    .is_none_or(|reason| !reason.contains(OTHER)),
            "{} leaks another subject: {:?} {:?}",
            selection.family,
            selection.selected_ids,
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

#[test]
fn subject_bundle_pg_the_pinned_families_are_carried_and_verified() {
    let Some((_guard, fixture)) = bundle_fixture_or_skip() else {
        return;
    };
    let bundle = build(&fixture, SELECTED);

    // The same pins as the SQLite slice: the selected class on every
    // per-subject family, the shared and unknown classes named, and the
    // same totals the mapping's live fixture yields.
    assert_eq!(bundle.manifest.backend, InventoryBackend::Postgres);
    assert_eq!(bundle.manifest.subject_id, SELECTED);
    for (name, class, selected) in [
        ("items", FamilyClass::Selected, 5),
        ("triples", FamilyClass::Selected, 2),
        ("tasks", FamilyClass::Selected, 1),
        ("failures", FamilyClass::Selected, 1),
        ("beliefs", FamilyClass::Selected, 1),
        ("tombstones", FamilyClass::UnknownLink, 0),
        ("assoc_edges", FamilyClass::InseparableShared, 0),
        ("persona_stable", FamilyClass::InseparableShared, 0),
    ] {
        let selection = bundle
            .manifest
            .families
            .iter()
            .find(|s| s.family == name)
            .unwrap_or_else(|| panic!("the manifest has no {name} family"));
        assert_eq!(selection.class, class, "{name} class");
        assert_eq!(selection.selected, selected, "{name} selected");
        assert!(
            selection.omission.is_some() && !selection.complete,
            "{name} names its omission"
        );
    }
    assert_eq!(
        (
            bundle.manifest.selected_total,
            bundle.manifest.omitted_total
        ),
        (10, 52),
        "the pinned totals"
    );
    let value: Value = serde_json::from_slice(&bundle.bytes).expect("parse the selection bytes");
    assert_eq!(
        value.get("backend").and_then(Value::as_str),
        Some("postgres")
    );
    bundle
        .verify_checksum()
        .expect("the bytes verify their own checksum");
    assert!(!bundle.manifest.complete);
    assert_no_other_subject(&bundle);
}

#[test]
fn subject_bundle_pg_the_same_subject_builds_identical_bytes_after_a_restart() {
    let Some((_guard, mut fixture)) = bundle_fixture_or_skip() else {
        return;
    };
    let before = build(&fixture, SELECTED);
    fixture.restart();
    let after = build(&fixture, SELECTED);

    assert_eq!(before.bytes, after.bytes);
    assert_eq!(
        before.manifest.content_sha256,
        after.manifest.content_sha256
    );
    assert_eq!(before.manifest.families, after.manifest.families);
    after
        .verify_checksum()
        .expect("the paired manifest verifies");
}

#[test]
fn subject_bundle_pg_a_changed_byte_fails_the_checksum() {
    let Some((_guard, fixture)) = bundle_fixture_or_skip() else {
        return;
    };
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
    SubjectBundleManifest {
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
fn subject_bundle_pg_a_capped_history_read_names_the_cap_and_stays_incomplete() {
    let Some((_guard, fixture)) = bundle_fixture_or_skip() else {
        return;
    };
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
    let tasks = bundle
        .manifest
        .families
        .iter()
        .find(|s| s.family == "tasks")
        .expect("the manifest has the tasks family");
    let reason = tasks.omission.as_deref().expect("the cap is named");
    assert!(
        reason.contains(&format!("capped at {HISTORY_LIMIT} rows")),
        "{reason}"
    );
    assert!(!tasks.complete && !bundle.manifest.complete);
    assert_no_other_subject(&bundle);
}

/// The sorted relative file names under `root`, or none when absent, for the
/// no-destination check.
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

#[test]
fn subject_bundle_pg_building_writes_no_destination() {
    let Some((_guard, mut fixture)) = bundle_fixture_or_skip() else {
        return;
    };
    // The placeholder ledger paths live under this bank's unique temp dir: a
    // destination write would have to create them.
    let base = std::env::temp_dir().join(&fixture.bank);
    let names_before = dir_names(&base);
    let before = build(&fixture, SELECTED);
    fixture.restart();
    let after = build(&fixture, SELECTED);

    assert_eq!(
        before.bytes, after.bytes,
        "the same subject builds identical bytes after a restart"
    );
    let names_after = dir_names(&base);
    assert_eq!(names_before, names_after, "no destination file appeared");
}
