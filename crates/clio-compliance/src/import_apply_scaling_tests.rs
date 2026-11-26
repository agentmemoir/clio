// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Test-only units owned by `import_apply.rs`: the bundle id index built once at
//! planner construction, its first-row-wins rule for duplicate ids, its coverage
//! of opaque rows, and the probe bound that keeps lookup work per id constant.
//! Fixture builders live in `import_fixture.rs`. Never compiled into production
//! builds.

use crate::bundle::ExportContentMode;
use crate::import_apply_seam::{probe_count, reset_probes};
use crate::import_fixture::{Harness, write_bundle};

use super::Planner;

/// Duplicate ids in one bundle keep the first row, and an opaque id is owned by
/// the bundle while staying absent from `bundle_item`.
#[test]
fn bundle_lookup_keeps_first_duplicate_and_owns_opaque_ids() {
    let h = Harness::new();
    let mut entities = h.bulk_entities(2);
    let shared = entities.items[0].id.clone();
    entities.items[1].id.clone_from(&shared);
    entities.items[0].gist = Some("first row wins".into());
    entities.items[1].gist = Some("second row loses".into());
    let mut opaque_entities = h.opaque_entities();
    let opaque = opaque_entities.opaque_items.remove(0);
    let opaque_id = opaque.id.clone();
    entities.opaque_items.push(opaque);

    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let value: serde_json::Value = serde_json::from_str(&bundle.text()).expect("bundle json");
    let parsed = crate::bundle::parse_bundle(&value).expect("bundle parses");
    let ctx = h.ctx();
    let args = h.args(&ctx, h.policy(), bundle.path(), true, false, None);
    let planner = Planner::new(&args, &parsed.envelope);

    assert_eq!(
        planner
            .bundle_item(&shared)
            .and_then(|item| item.gist.as_deref()),
        Some("first row wins")
    );
    assert!(planner.bundle_owns(&shared));
    assert!(planner.bundle_item(&opaque_id).is_none());
    assert!(planner.bundle_owns(&opaque_id));
}

/// The index must answer each lookup with a small, fixed number of probes: a
/// walk of the bundle arrays performs none, and a scan that hashes per row
/// exceeds the ceiling.
fn assert_bounded_lookup_probes(probes: usize, ids: usize, kind: &str) {
    let lookups = 2 * ids;
    assert!(
        probes >= lookups,
        "{probes} index probes for {lookups} {kind} lookups over {ids} ids: \
         at least one lookup walked the bundle arrays instead of probing the index"
    );
    assert!(
        probes <= 2 * lookups,
        "{probes} index probes for {lookups} {kind} lookups over {ids} ids: \
         lookup work is not bounded per id"
    );
}

/// Lookup work stays bounded at scale: the index holds one entry per bundled
/// item, every present id resolves, every absent id is rejected, and each
/// lookup costs at least one and at most two hash probes.
#[test]
fn planner_lookup_work_stays_bounded_at_thousands_of_ids() {
    const COUNT: usize = 4_000;
    let h = Harness::new();
    let entities = h.bulk_entities(COUNT);
    let ids: Vec<String> = entities.items.iter().map(|item| item.id.clone()).collect();

    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let value: serde_json::Value = serde_json::from_str(&bundle.text()).expect("bundle json");
    let parsed = crate::bundle::parse_bundle(&value).expect("bundle parses");
    let ctx = h.ctx();
    let args = h.args(&ctx, h.policy(), bundle.path(), true, false, None);
    let planner = Planner::new(&args, &parsed.envelope);

    assert_eq!(planner.bundle_items.len(), COUNT);
    assert!(planner.bundle_opaque.is_empty());

    reset_probes();
    for id in &ids {
        assert!(
            planner.bundle_item(id).is_some(),
            "bundled row `{id}` is missing from the index"
        );
        assert!(planner.bundle_owns(id));
    }
    let present = probe_count();
    assert_bounded_lookup_probes(present, COUNT, "present");

    reset_probes();
    for i in 0..COUNT {
        let absent = format!("absent-{i}");
        assert!(
            planner.bundle_item(&absent).is_none(),
            "absent row `{absent}` resolved"
        );
        assert!(!planner.bundle_owns(&absent));
    }
    let absent = probe_count();
    assert_bounded_lookup_probes(absent, COUNT, "absent");
    // Measured, not gated: the probe bound above is the claim, and this line
    // keeps the actual count visible in a run log.
    println!("index probes: present {present}, absent {absent}, ids {COUNT}");
}
