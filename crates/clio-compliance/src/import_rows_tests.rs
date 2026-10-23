// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Test-only units owned by `import_rows.rs`: the derived id a cross-bank
//! restore writes into a carrier and its row, plus the existence caches
//! `preplan` keys on it. Fixtures: `import_fixture.rs`; repro:
//! `import_rekey_tests.rs`. Never compiled into production builds.

use clio_store::BatchWrite;
use clio_types::{Actor, Bank, OpContext};

use super::Planner;
use crate::bundle::ExportContentMode;
use crate::import::import;
use crate::import_fixture::{Harness, write_bundle};

const SOURCE_BANK: &str = "exported-bank";

/// Every staged `Commit*` row as `(kind, carrier item id, row item_id)`, in
/// staging order: triples, then tasks, then failures.
fn staged_rows(planner: &Planner<'_>) -> Vec<(&'static str, String, String)> {
    planner
        .staged
        .iter()
        .filter_map(|write| match write {
            BatchWrite::CommitTriple { item, edge, .. } => {
                Some(("triple", item.id.clone(), edge.item_id.clone()))
            }
            BatchWrite::CommitTaskUpsert { item, record, .. } => {
                Some(("task", item.id.clone(), record.item_id.clone()))
            }
            BatchWrite::CommitFailureRecord { item, record, .. } => {
                Some(("failure", item.id.clone(), record.item_id.clone()))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn cross_bank_carriers_and_rows_stage_under_derived_ids() {
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
    planner.run().expect("planning succeeds");

    let suffix = format!("#{}", h.bank);
    let derived = |bundle_id: &str| format!("{bundle_id}{suffix}");
    let triple = &entities.triples[0];
    let task = &entities.tasks[0];
    let failure = &entities.failures[0];

    // `preplan` keyed its existence lookups on the written (derived) row ids.
    let caches = (&planner.edge_seen, &planner.failure_exists);
    assert_eq!(caches.0.len(), 1);
    assert!(caches.0.contains_key(&format!("{}{suffix}", triple.id)));
    assert!(caches.1.contains_key(&format!("{}{suffix}", failure.id)));

    let row = |kind: &'static str, item_id: &str| (kind, derived(item_id), derived(item_id));
    assert_eq!(
        staged_rows(&planner),
        vec![
            row("triple", &triple.item_id),
            row("task", &task.item_id),
            row("failure", &failure.item_id),
        ]
    );
}
