// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Inspector predicate-compiler tests (path-excluded from the coverage gate).

use super::{InspectValidity, ItemInspectFilter, compile_inspect};
use crate::model::InspectSource;

#[test]
fn compile_rejects_bad_inputs() {
    let mut filter = ItemInspectFilter::new("");
    assert!(compile_inspect(&filter, '?').is_err());
    filter.bank_id = "b".into();
    filter.limit = 0;
    assert!(compile_inspect(&filter, '?').is_err());
    filter.limit = 10;
    filter.epistemic_kind = Some("guess".into());
    assert!(compile_inspect(&filter, '?').is_err());
}

#[test]
fn compile_orders_predicates_and_binds_values() {
    let mut filter = ItemInspectFilter::new("bank-a");
    filter.category = Some("persona".into());
    filter.epistemic_kind = Some("fact".into());
    filter.validity = InspectValidity::Active;
    filter.time_start = Some("2024-01-01".into());
    filter.time_end = Some("2024-12-31".into());
    let compiled = compile_inspect(&filter, '$').expect("compile");
    assert_eq!(
        compiled.where_sql,
        "bank_id = $1 AND category = $2 AND epistemic_kind = $3 \
         AND tx_until IS NULL AND created_at >= $4 AND created_at < $5 \
         AND discarded_at IS NULL"
    );
    assert_eq!(
        compiled.params,
        ["bank-a", "persona", "fact", "2024-01-01", "2025-01-01"]
    );
}

#[test]
fn compile_keeps_timestamp_end_inclusive() {
    let mut filter = ItemInspectFilter::new("bank-a");
    filter.time_end = Some("2024-12-31T23:00:00Z".into());
    let compiled = compile_inspect(&filter, '?').expect("compile");
    assert_eq!(
        compiled.where_sql,
        "bank_id = ?1 AND created_at <= ?2 AND discarded_at IS NULL"
    );
    assert_eq!(compiled.params, ["bank-a", "2024-12-31T23:00:00Z"]);
}

#[test]
fn compile_advances_a_leap_day_bound_to_the_next_date() {
    let mut filter = ItemInspectFilter::new("bank-a");
    filter.time_end = Some("2024-02-29".into());
    let compiled = compile_inspect(&filter, '?').expect("compile");
    assert_eq!(compiled.params, ["bank-a", "2024-03-01"]);
}

#[test]
fn compile_invalidated_keeps_discarded_when_asked() {
    let mut filter = ItemInspectFilter::new("b");
    filter.validity = InspectValidity::Invalidated;
    filter.include_discarded = true;
    let compiled = compile_inspect(&filter, '?').expect("compile");
    assert_eq!(compiled.where_sql, "bank_id = ?1 AND tx_until IS NOT NULL");
    assert_eq!(compiled.params, ["b"]);
}

#[test]
fn compile_omits_source_type_when_absent() {
    let filter = ItemInspectFilter::new("b");
    let compiled = compile_inspect(&filter, '?').expect("compile");
    assert_eq!(compiled.where_sql, "bank_id = ?1 AND discarded_at IS NULL");
    assert_eq!(compiled.params, ["b"]);
}

#[test]
fn compile_binds_belief_source_type_values() {
    let mut filter = ItemInspectFilter::new("b");
    filter.epistemic_kind = Some("belief".into());
    filter.source_type = Some(InspectSource::AgentInferred);
    let compiled = compile_inspect(&filter, '?').expect("compile");
    assert_eq!(
        compiled.where_sql,
        "bank_id = ?1 AND epistemic_kind = ?2 AND source_type = ?3 \
         AND discarded_at IS NULL"
    );
    assert_eq!(compiled.params, ["b", "belief", "agent_inferred"]);
}

#[test]
fn compile_fact_source_type_is_an_unbound_null_clause() {
    let mut filter = ItemInspectFilter::new("b");
    filter.source_type = Some(InspectSource::Fact);
    let compiled = compile_inspect(&filter, '?').expect("compile");
    assert_eq!(
        compiled.where_sql,
        "bank_id = ?1 AND source_type IS NULL AND discarded_at IS NULL"
    );
    assert_eq!(compiled.params, ["b"]);
}
