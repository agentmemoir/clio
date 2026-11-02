// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Test-only units owned by `import_apply_opaque.rs`: the same-bank opaque
//! staging path. The cross-bank refusal has its own test in
//! `import_apply_tests.rs`, where the planner-level table lives. Never
//! compiled into production builds.

use clio_store::BatchWrite;
use clio_types::{Actor, Bank, OpContext};

use super::Planner;
use crate::bundle::ExportContentMode;
use crate::import_fixture::{Harness, write_bundle};

/// A manifest that names exactly the target bank keeps the exported id, so
/// the sealed bytes stay readable under the row they were sealed for.
#[test]
fn same_bank_opaque_row_keeps_its_exported_id() {
    let h = Harness::new();
    let entities = h.opaque_entities();
    let exported_id = entities.opaque_items[0].id.clone();
    let bundle = write_bundle(&entities, ExportContentMode::CiphertextBackup);
    let value: serde_json::Value = serde_json::from_str(&bundle.text()).expect("bundle json");
    let parsed = crate::bundle::parse_bundle(&value).expect("bundle parses");
    let ctx = OpContext::new(Bank::new("exported-bank"), Actor::Agent);
    let args = h.args(&ctx, h.policy(), bundle.path(), false, false, None);
    let mut planner = Planner::new(&args, &parsed.envelope);

    planner.plan_opaque().expect("same-bank planning succeeds");

    let created = planner.staged.iter().find_map(|write| match write {
        BatchWrite::PutOpaqueItem { item, .. } => Some(item.id.clone()),
        _ => None,
    });
    assert_eq!(created.as_deref(), Some(exported_id.as_str()));
    assert_eq!(planner.create, 1);
}
