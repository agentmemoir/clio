// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the cross-bank re-key regression suite: a
//! bundle imported once into its own bank and then into a second bank, where
//! the deployment-unique primary keys force the second restore to derive ids.
//!
//! ## Owns
//! - The two-pass repro (`exported-bank` then `ctx.bank`) and its derived ids
//! - Source-bank immutability and third-pass idempotency assertions
//!
//! ## Does not own
//! - Fixture builders (see `import_fixture.rs`)
//! - Same-bank idempotency (see `import_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use clio_store::{AssocEdgeRecord, HistoryStore, ItemInspectFilter, Store};
use clio_types::{Actor, Bank, OpContext};

use crate::bundle::{
    BUNDLE_FORMAT, BUNDLE_FORMAT_VERSION, BundleEnvelope, CompletenessManifest, ExportContentMode,
    ExportEntities, OpaqueItem, entities_checksum,
};
use crate::import::{ImportReport, import};
use crate::import_fixture::{Harness, TempBundle, unique, write_bundle};

const SOURCE_BANK: &str = "exported-bank";

fn count_in_bank(h: &Harness, bank: &str) -> usize {
    h.store.count_items(&ItemInspectFilter::new(bank)).unwrap()
}

fn source_ctx() -> OpContext {
    OpContext::new(Bank::new(SOURCE_BANK), Actor::Agent)
}

fn import_into(h: &Harness, bundle: &TempBundle, ctx: &OpContext) -> ImportReport {
    import(&h.args(ctx, h.policy(), bundle.path(), false, false, None))
        .unwrap_or_else(|e| panic!("import into {} failed: {e:?}", ctx.bank.as_str()))
}

/// Import the bundle into its own bank, then into `h.bank`.
fn import_twice(h: &Harness, bundle: &TempBundle) -> (ImportReport, ImportReport) {
    let first = import_into(h, bundle, &source_ctx());
    let second = import_into(h, bundle, &h.ctx());
    (first, second)
}

#[test]
fn import_into_second_bank_derives_ids() {
    let h = Harness::new();
    let entities = h.full_entities();
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let (first, second) = import_twice(&h, &bundle);

    assert_eq!((first.would_create, first.would_skip), (11, 0));
    assert_eq!((second.would_create, second.would_skip), (11, 0));
    assert_eq!((second.would_reject, second.would_overwrite), (0, 0));
    assert!(second.applied);

    let suffix = format!("#{}", h.bank);
    assert_eq!(second.created_ids.len(), 4);
    assert!(
        second.created_ids.iter().all(|id| id.ends_with(&suffix)),
        "{:?}",
        second.created_ids
    );

    let semantic = entities.items.first().unwrap();
    let derived = format!("{}{suffix}", semantic.id);
    let restored = h
        .store
        .get_memory_item(&h.bank, &derived)
        .unwrap()
        .expect("derived id is readable from the target bank");
    assert_eq!(
        (restored.id.as_str(), restored.bank_id.as_str()),
        (derived.as_str(), h.bank.as_str())
    );
    assert_eq!(restored.gist, semantic.gist);
    assert_eq!(count_in_bank(&h, &h.bank), 4);

    assert_eq!(count_in_bank(&h, SOURCE_BANK), 4);
    let original = h
        .store
        .get_memory_item(SOURCE_BANK, &semantic.id)
        .unwrap()
        .expect("source row is untouched");
    assert_eq!(
        (original.id.as_str(), original.bank_id.as_str()),
        (semantic.id.as_str(), SOURCE_BANK)
    );
    assert!(
        h.store
            .get_memory_item(SOURCE_BANK, &derived)
            .unwrap()
            .is_none()
    );
    assert_eq!(h.store.list_triples(&h.bank).unwrap().len(), 1);
    assert_eq!(h.store.list_triples(SOURCE_BANK).unwrap().len(), 1);
    assert_eq!(h.store.list_failures(&h.bank, 100).unwrap().len(), 1);
    assert_eq!(h.store.list_failures(SOURCE_BANK, 100).unwrap().len(), 1);
}

#[test]
fn reimport_into_same_second_bank_skips() {
    let h = Harness::new();
    let bundle = write_bundle(&h.full_entities(), ExportContentMode::DsarPlaintext);
    let (_, second) = import_twice(&h, &bundle);
    assert_eq!((second.would_create, second.would_skip), (11, 0));
    let target_items = count_in_bank(&h, &h.bank);

    let third = import_into(&h, &bundle, &h.ctx());
    assert_eq!((third.would_create, third.would_skip), (0, 11));
    assert!(third.created_ids.is_empty(), "{:?}", third.created_ids);
    assert_eq!(count_in_bank(&h, &h.bank), target_items);
    assert_eq!(h.store.list_triples(&h.bank).unwrap().len(), 1);
    assert_eq!(count_in_bank(&h, SOURCE_BANK), 4);
}

/// One opaque bundle row with a caller-chosen id (the fixture row carries the
/// `ciphertext_backup` content-envelope shape).
fn opaque_row(h: &Harness, id: &str) -> OpaqueItem {
    let mut row = h
        .opaque_entities()
        .opaque_items
        .into_iter()
        .next()
        .expect("opaque fixture row");
    row.id = id.to_owned();
    row
}

/// Bundle writer with a caller-chosen manifest `bank_ids` (the shared fixture
/// always emits exactly one bank id, so provenance edge cases need this).
fn write_bundle_for_banks(
    entities: &ExportEntities,
    mode: ExportContentMode,
    bank_ids: Vec<String>,
) -> TempBundle {
    let envelope = BundleEnvelope {
        format: BUNDLE_FORMAT.to_owned(),
        format_version: BUNDLE_FORMAT_VERSION,
        content_mode: mode,
        manifest: CompletenessManifest {
            format_version: BUNDLE_FORMAT_VERSION,
            content_mode: mode,
            bank_ids,
            counts_by_type: entities.counts_by_type(),
            counts_by_category: vec![],
            content_sha256: entities_checksum(entities),
            filter: serde_json::json!({}),
            generated_at: "2026-06-01T00:00:00Z".into(),
            complete: true,
            omissions: vec![],
        },
        entities: entities.clone(),
    };
    let path = std::env::temp_dir().join(unique("am-rekey-bundle"));
    std::fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
    TempBundle(path)
}

/// A triple whose `snapshot_ref` / `gist_ref` point at rows the bundle also
/// carries must re-key those pointers; a stale pointer fails the whole import
/// (`ItemNotFound: snapshot_ref ... not found in bank`).
#[test]
fn triple_refs_follow_bundle_items_into_the_second_bank() {
    let h = Harness::new();
    let mut entities = h.full_entities();
    let snapshot_source = entities.items[0].id.clone();
    let gist_source = entities.items[2].id.clone();
    entities.triples[0].snapshot_ref = Some(snapshot_source.clone());
    entities.triples[0].gist_ref = Some(gist_source.clone());
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);

    let first = import_into(&h, &bundle, &source_ctx());
    assert_eq!((first.would_create, first.would_skip), (11, 0));
    let second = import_into(&h, &bundle, &h.ctx());
    assert_eq!((second.would_create, second.would_skip), (11, 0));

    let suffix = format!("#{}", h.bank);
    let edges = h.store.list_triples(&h.bank).unwrap();
    assert_eq!(edges.len(), 1);
    let edge = &edges[0];
    assert_eq!(edge.id, format!("{}{suffix}", entities.triples[0].id));
    let expected_snapshot = format!("{snapshot_source}{suffix}");
    let expected_gist = format!("{gist_source}{suffix}");
    assert_eq!(
        edge.snapshot_ref.as_deref(),
        Some(expected_snapshot.as_str())
    );
    assert_eq!(edge.gist_ref.as_deref(), Some(expected_gist.as_str()));
    for ref_id in [&expected_snapshot, &expected_gist] {
        assert!(
            h.store.get_memory_item(&h.bank, ref_id).unwrap().is_some(),
            "re-keyed ref `{ref_id}` must resolve to a row in the target bank"
        );
    }
    let originals = h.store.list_triples(SOURCE_BANK).unwrap();
    assert_eq!(
        originals[0].snapshot_ref.as_deref(),
        Some(snapshot_source.as_str()),
        "the source bank's own refs are not rewritten"
    );
}

/// `ciphertext_backup` bundles put every row in `opaque_items` and leave
/// `items` empty; assoc edges over those rows must still be re-keyed, or the
/// target bank gets edges pointing at source-bank ids.
#[test]
fn ciphertext_backup_assoc_edges_point_at_rekeyed_opaque_rows() {
    let h = Harness::new();
    let left = unique("opq");
    let right = unique("opq");
    let entities = ExportEntities {
        opaque_items: vec![opaque_row(&h, &left), opaque_row(&h, &right)],
        assoc_edges: vec![AssocEdgeRecord {
            id: unique("ae"),
            bank_id: SOURCE_BANK.into(),
            source_id: left.clone(),
            target_id: right.clone(),
            assoc_kind: clio_store::AssocKind::Explicit,
            relationship: Some("relates".into()),
            weight: 0.4,
            last_updated_at: "2026-06-01T00:00:00Z".into(),
        }],
        ..ExportEntities::default()
    };
    let bundle = write_bundle(&entities, ExportContentMode::CiphertextBackup);
    let first = import_into(&h, &bundle, &source_ctx());
    assert_eq!(first.would_create, 3);
    let second = import_into(&h, &bundle, &h.ctx());
    assert_eq!(second.would_create, 3, "2 opaque rows + 1 assoc edge");

    let suffix = format!("#{}", h.bank);
    let derived_left = format!("{left}{suffix}");
    let derived_right = format!("{right}{suffix}");
    let edges = h.store.list_assoc_edges(&h.bank, None).unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].source_id, derived_left);
    assert_eq!(edges[0].target_id, derived_right);
    for endpoint in [&derived_left, &derived_right] {
        assert!(
            h.store
                .get_item_identity(&h.bank, endpoint)
                .unwrap()
                .is_some(),
            "assoc endpoint `{endpoint}` must exist in the target bank"
        );
    }
}

/// `OpaqueItem` has no bank provenance, so only a single-bank manifest naming
/// the target bank is provably same-bank; a multi-bank or empty `bank_ids` must
/// fail closed to the derived id. Otherwise the opaque upsert
/// (`ON CONFLICT (id) DO UPDATE SET bank_id = EXCLUDED.bank_id`) moves the
/// source bank's row into the target bank.
#[test]
fn opaque_ids_fail_closed_outside_a_single_bank_manifest() {
    let h = Harness::new();
    let multi_id = unique("opq");
    let empty_id = unique("opq");
    let seed = write_bundle(
        &ExportEntities {
            opaque_items: vec![opaque_row(&h, &multi_id), opaque_row(&h, &empty_id)],
            ..ExportEntities::default()
        },
        ExportContentMode::CiphertextBackup,
    );
    import_into(&h, &seed, &source_ctx());

    let multi = write_bundle_for_banks(
        &ExportEntities {
            opaque_items: vec![opaque_row(&h, &multi_id)],
            ..ExportEntities::default()
        },
        ExportContentMode::CiphertextBackup,
        vec![SOURCE_BANK.to_owned(), h.bank.clone()],
    );
    assert_eq!(import_into(&h, &multi, &h.ctx()).would_create, 1);

    let empty = write_bundle_for_banks(
        &ExportEntities {
            opaque_items: vec![opaque_row(&h, &empty_id)],
            ..ExportEntities::default()
        },
        ExportContentMode::CiphertextBackup,
        vec![],
    );
    assert_eq!(import_into(&h, &empty, &h.ctx()).would_create, 1);

    let suffix = format!("#{}", h.bank);
    for id in [&multi_id, &empty_id] {
        let derived = format!("{id}{suffix}");
        assert!(
            h.store
                .get_item_identity(&h.bank, &derived)
                .unwrap()
                .is_some(),
            "derived id `{derived}` is written in the target bank"
        );
        assert!(
            h.store.get_item_identity(&h.bank, id).unwrap().is_none(),
            "the unkeyed source id must not appear in the target bank"
        );
        assert!(
            h.store
                .get_item_identity(SOURCE_BANK, id)
                .unwrap()
                .is_some(),
            "source row `{id}` keeps its original id and bank"
        );
    }
    assert_eq!(count_in_bank(&h, SOURCE_BANK), 2);
    assert_eq!(count_in_bank(&h, &h.bank), 2);
}
