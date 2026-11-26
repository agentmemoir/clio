// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Test-only units owned by `import_apply.rs`: the planner branches that decide
//! create / skip / overwrite / reject for a real bundle — the note cap, the
//! force audit, idempotent skips, admission rejection, oversized-gist skips on
//! plain rows and on deferred carriers, assoc-edge re-keying, and tombstone
//! provenance. Fixtures live in `import_fixture.rs`. Never compiled into
//! production builds.

use clio_admission::AdmissionPolicy;
use clio_config::ranking::RankingEnv;
use clio_store::BatchWrite;

use super::{CarrierBlock, Planner};
use crate::bundle::ExportContentMode;
use crate::import::import;
use crate::import_fixture::{Harness, write_bundle};

/// Every staged `import_force` audit row's attributed item id, sorted.
fn force_audit_ids(planner: &Planner<'_>) -> Vec<String> {
    let mut ids: Vec<String> = planner
        .staged
        .iter()
        .filter_map(|write| match write {
            BatchWrite::AuditEvent {
                operation, item_id, ..
            } if operation == "import_force" => item_id.clone(),
            _ => None,
        })
        .collect();
    ids.sort();
    ids
}

#[test]
fn note_cap_keeps_nine_notes_and_one_truncation_marker() {
    let h = Harness::new();
    let entities = h.bulk_entities(1);
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let value: serde_json::Value = serde_json::from_str(&bundle.text()).expect("bundle json");
    let parsed = crate::bundle::parse_bundle(&value).expect("bundle parses");
    let ctx = h.ctx();
    let args = h.args(&ctx, h.policy(), bundle.path(), false, false, None);
    let mut planner = Planner::new(&args, &parsed.envelope);

    for i in 0..20 {
        planner.note(format!("note {i}"));
    }

    assert_eq!(planner.notes.len(), 10, "{:?}", planner.notes);
    assert_eq!(planner.notes[8], "note 8");
    assert_eq!(planner.notes[9], "(further notes truncated)");
}

#[test]
fn force_overwrite_stages_masked_upsert_and_one_audit_per_existing_item() {
    let h = Harness::new();
    let entities = h.full_entities();
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let ctx = h.ctx();
    import(&h.args(&ctx, h.policy(), bundle.path(), false, false, None))
        .expect("first import succeeds");

    let value: serde_json::Value = serde_json::from_str(&bundle.text()).expect("bundle json");
    let parsed = crate::bundle::parse_bundle(&value).expect("bundle parses");
    let args = h.args(
        &ctx,
        h.policy(),
        bundle.path(),
        false,
        true,
        Some("ops restore"),
    );
    let mut planner = Planner::new(&args, &parsed.envelope);
    planner.preplan().expect("preplan succeeds");
    planner.plan_items().expect("item planning succeeds");

    assert_eq!(planner.overwrite, 4, "every bundle item already exists");
    assert_eq!(planner.skip, 0);
    let upserts = planner
        .staged
        .iter()
        .filter(|write| matches!(write, BatchWrite::UpsertMemoryItem { .. }))
        .count();
    assert_eq!(upserts, 4);
    let mut expected: Vec<String> = entities
        .items
        .iter()
        .map(|item| format!("{}#{}", item.id, h.bank))
        .collect();
    expected.sort();
    assert_eq!(force_audit_ids(&planner), expected);
}

#[test]
fn existing_items_without_force_are_skipped_without_staging() {
    let h = Harness::new();
    let entities = h.full_entities();
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let ctx = h.ctx();
    import(&h.args(&ctx, h.policy(), bundle.path(), false, false, None))
        .expect("first import succeeds");

    let value: serde_json::Value = serde_json::from_str(&bundle.text()).expect("bundle json");
    let parsed = crate::bundle::parse_bundle(&value).expect("bundle parses");
    let args = h.args(&ctx, h.policy(), bundle.path(), false, false, None);
    let mut planner = Planner::new(&args, &parsed.envelope);
    planner.preplan().expect("preplan succeeds");
    planner.plan_items().expect("item planning succeeds");

    assert_eq!(planner.skip, 4);
    assert_eq!(planner.overwrite, 0);
    assert!(planner.staged.is_empty(), "{:?}", planner.staged);
}

#[test]
fn admission_rejection_is_counted_and_sampled_not_staged() {
    let h = Harness::new();
    let entities = h.reject_entities();
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let mut env = RankingEnv::system_defaults();
    env.theta_admit = 1.0;
    let policy = AdmissionPolicy::new(&env);
    let ctx = h.ctx();
    let args = h.args(&ctx, policy, bundle.path(), false, false, None);
    let envelope = parsed_envelope(&bundle);
    let mut planner = Planner::new(&args, &envelope);
    planner.plan_items().expect("item planning succeeds");

    assert_eq!(planner.reject, 1);
    assert_eq!(planner.samples.len(), 1, "{:?}", planner.samples);
    assert_eq!(planner.create, 0);
    assert!(planner.staged.is_empty(), "{:?}", planner.staged);
}

#[test]
fn oversized_gist_item_is_reported_and_never_staged() {
    let h = Harness::new();
    let mut entities = h.bulk_entities(1);
    entities.items[0].gist = Some("g".repeat(300));
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let ctx = h.ctx();
    let args = h.args(&ctx, h.policy(), bundle.path(), false, false, None);
    let envelope = parsed_envelope(&bundle);
    let mut planner = Planner::new(&args, &envelope);
    planner.plan_items().expect("item planning succeeds");

    assert_eq!(planner.skip_invalid, 1);
    assert_eq!(planner.create, 0);
    assert!(planner.staged.is_empty(), "{:?}", planner.staged);
    let skipped = planner.skipped_invalid.first().expect("one skipped row");
    assert_eq!(skipped.code, "out_of_range");
    assert!(skipped.reason.contains("250"), "{skipped:?}");
}

#[test]
fn oversized_gist_on_a_deferred_carrier_blocks_that_carrier() {
    let h = Harness::new();
    let mut entities = h.full_entities();
    let mut carrier = entities.items.remove(1);
    let carrier_id = carrier.id.clone();
    carrier.gist = Some("g".repeat(300));
    entities.items = vec![carrier];
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let ctx = h.ctx();
    let args = h.args(&ctx, h.policy(), bundle.path(), false, false, None);
    let envelope = parsed_envelope(&bundle);
    let mut planner = Planner::new(&args, &envelope);
    planner.preplan().expect("preplan succeeds");
    planner.plan_items().expect("item planning succeeds");

    assert_eq!(planner.skip_invalid, 1);
    assert!(
        planner.deferred.contains(&carrier_id),
        "{:?}",
        planner.deferred
    );
    let written = format!("{carrier_id}#{}", h.bank);
    let block = planner.blocked_carriers.get(&written).unwrap_or_else(|| {
        panic!(
            "carrier is missing from blocked_carriers: {:?}",
            planner.blocked_carriers.keys().collect::<Vec<_>>()
        )
    });
    match block {
        CarrierBlock::Invalid(reason) => assert!(reason.contains("250"), "{reason}"),
        CarrierBlock::Rejected(_) => {
            panic!("a gist skip must not be reported as an admission rejection");
        }
    }
}

#[test]
fn assoc_edge_rewrites_endpoints_the_bundle_owns() {
    let h = Harness::new();
    let mut entities = h.full_entities();
    let source = entities.items[0].id.clone();
    let target = entities.items[1].id.clone();
    entities.assoc_edges[0].source_id.clone_from(&source);
    entities.assoc_edges[0].target_id.clone_from(&target);
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let ctx = h.ctx();
    let args = h.args(&ctx, h.policy(), bundle.path(), false, false, None);
    let envelope = parsed_envelope(&bundle);
    let mut planner = Planner::new(&args, &envelope);
    planner.plan_assoc().expect("assoc planning succeeds");

    assert_eq!(planner.create, 1);
    let edge = planner
        .staged
        .iter()
        .find_map(|write| match write {
            BatchWrite::PutAssocEdge { edge, .. } => Some(edge),
            _ => None,
        })
        .expect("one assoc edge is staged");
    assert_eq!(edge.source_id, format!("{source}#{}", h.bank));
    assert_eq!(edge.target_id, format!("{target}#{}", h.bank));
}

#[test]
fn existing_assoc_edge_is_skipped_or_overwritten_by_force() {
    let h = Harness::new();
    let entities = h.full_entities();
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let ctx = h.ctx();
    import(&h.args(&ctx, h.policy(), bundle.path(), false, false, None))
        .expect("first import succeeds");

    let envelope = parsed_envelope(&bundle);
    let plain = h.args(&ctx, h.policy(), bundle.path(), false, false, None);
    let mut skip = Planner::new(&plain, &envelope);
    skip.plan_assoc().expect("assoc planning succeeds");
    assert_eq!(skip.skip, 1);
    assert!(skip.staged.is_empty(), "{:?}", skip.staged);

    let forced = h.args(&ctx, h.policy(), bundle.path(), false, true, None);
    let mut overwrite = Planner::new(&forced, &envelope);
    overwrite.plan_assoc().expect("assoc planning succeeds");
    assert_eq!(overwrite.overwrite, 1);
    assert_eq!(
        force_audit_ids(&overwrite).len(),
        1,
        "{:?}",
        overwrite.staged
    );
}

#[test]
fn tombstones_are_skipped_with_an_export_provenance_note() {
    let h = Harness::new();
    let entities = Harness::tombstone_entities();
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let ctx = h.ctx();
    let args = h.args(&ctx, h.policy(), bundle.path(), false, false, None);
    let envelope = parsed_envelope(&bundle);
    let mut planner = Planner::new(&args, &envelope);
    planner.plan_tombstones();

    assert_eq!(planner.skip, 1);
    assert!(
        planner.notes.iter().any(|note| note.contains("tombstones")),
        "{:?}",
        planner.notes
    );
}

/// Parse a fixture bundle the way the import entrypoint does.
fn parsed_envelope(bundle: &crate::import_fixture::TempBundle) -> crate::bundle::BundleEnvelope {
    let value: serde_json::Value = serde_json::from_str(&bundle.text()).expect("bundle json");
    crate::bundle::parse_bundle(&value)
        .expect("bundle parses")
        .envelope
}
