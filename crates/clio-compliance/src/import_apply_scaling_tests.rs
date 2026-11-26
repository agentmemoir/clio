// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Test-only units owned by `import_apply.rs`: the bundle id index built once at
//! planner construction, its first-row-wins rule for duplicate ids, and its
//! coverage of opaque rows. Fixture builders live in `import_fixture.rs`. Never
//! compiled into production builds.

use std::collections::{HashMap, HashSet};

use crate::bundle::ExportContentMode;
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

/// The index holds one entry per item and is built before planning starts, so a
/// lookup is a hash probe instead of a walk over the item array.
#[test]
fn planner_indexes_every_item_and_rejects_thousands_of_absent_ids() {
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

    // A hash table answers a key lookup in constant time on average, so the
    // container types are the structural proof that lookup cost does not follow
    // the number of items.
    let items: &HashMap<_, _> = &planner.bundle_items;
    let opaque: &HashSet<_> = &planner.bundle_opaque;
    assert_eq!(items.len(), COUNT);
    assert!(opaque.is_empty());

    for id in &ids {
        assert!(
            planner.bundle_item(id).is_some(),
            "bundled row `{id}` is missing from the index"
        );
        assert!(planner.bundle_owns(id));
    }
    for i in 0..COUNT {
        let absent = format!("absent-{i}");
        assert!(
            planner.bundle_item(&absent).is_none(),
            "absent row `{absent}` resolved"
        );
        assert!(!planner.bundle_owns(&absent));
    }
    assert_eq!(planner.bundle_items.len(), COUNT);
    assert_eq!(planner.bundle_opaque.len(), 0);
}
