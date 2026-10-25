// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Test-only units owned by `import_apply.rs`: the cross-bank id derivation
//! and the item create planned under the derived id. Fixture builders live in
//! `import_fixture.rs`, carrier-backed rows in `import_rows_tests.rs`. Never
//! compiled into production builds.

use std::borrow::Cow;

use clio_store::BatchWrite;
use clio_types::{Actor, Bank, OpContext};

use super::{Planner, import_id};
use crate::bundle::ExportContentMode;
use crate::import::import;
use crate::import_fixture::{Harness, write_bundle};

const SOURCE_BANK: &str = "exported-bank";

#[test]
fn same_bank_id_is_borrowed_and_unchanged() {
    let id = import_id("bank-a", "bank-a", "itm-1");
    assert!(matches!(id, Cow::Borrowed("itm-1")), "{id:?}");
}

#[test]
fn cross_bank_id_is_derived_from_target_bank_only() {
    assert_eq!(import_id("bank-b", "bank-a", "itm-1"), "itm-1#bank-b");
    assert_eq!(
        import_id("bank-b", "bank-a", "itm-1"),
        import_id("bank-b", "bank-c", "itm-1")
    );
}

#[test]
fn id_derivation_is_deterministic_across_calls() {
    let first = import_id("bank-b", "bank-a", "itm-1").into_owned();
    let again = import_id("bank-b", "bank-a", "itm-1").into_owned();
    assert_eq!(first, again);
    assert_eq!(first, import_id("bank-b", "bank-a", "itm-1"));
}

/// The bundle's rows exist only in the exported bank, so the target-bank plan
/// must create the item under `{id}#{bank}` while its carrier stays deferred
/// under the bundle id the row executor re-inserts.
#[test]
fn cross_bank_item_creates_under_derived_id_defers_by_bundle_id() {
    let h = Harness::new();
    let entities = h.full_entities();
    let bundle = write_bundle(&entities, ExportContentMode::DsarPlaintext);
    let source_ctx = OpContext::new(Bank::new(SOURCE_BANK), Actor::Agent);
    import(&h.args(&source_ctx, h.policy(), bundle.path(), false, false, None))
        .expect("source-bank import succeeds");

    let value: serde_json::Value = serde_json::from_str(&bundle.text()).expect("bundle json");
    let parsed = crate::bundle::parse_bundle(&value).expect("bundle parses");
    let ctx = h.ctx();
    let args = h.args(&ctx, h.policy(), bundle.path(), true, false, None);
    let mut planner = Planner::new(&args, &parsed.envelope);
    planner.preplan().expect("preplan succeeds");
    planner.plan_items().expect("item planning succeeds");

    let derived = format!("{}#{}", entities.items[0].id, h.bank);
    let created = planner.staged.iter().find_map(|write| match write {
        BatchWrite::CreateItem { item, .. } => Some(item.id.clone()),
        _ => None,
    });
    assert_eq!(created.as_deref(), Some(derived.as_str()));
    // The item plus its three gated carriers count; only one stages a create.
    assert_eq!(planner.create, 4);

    let carrier = &entities.triples[0].item_id;
    let deferred = &planner.deferred;
    assert!(deferred.contains(carrier), "{deferred:?}");
    assert!(!deferred.contains(&derived), "{deferred:?}");
}
