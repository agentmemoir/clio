// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the published `retrieve` schema text for the per-hit
//! `scores.age_factor` and `scores.trust_factor` contracts and for the
//! provenance floor: the tool description states what each factor is, the
//! `min_scores.final` floor states that it applies to the demoted value, and
//! the `provenance_floor` property states the weakest rung kept and that a hit
//! below it is removed rather than ranked lower.
//!
//! ## Owns
//! - The `scores.age_factor` and `scores.trust_factor` description assertions
//!   over the published `retrieve` tool definition
//! - The `final` floor's demoted-value consequence assertion over the
//!   published `min_scores` schema
//! - The `provenance_floor` rung, membership-only, and omitted-default
//!   assertions over the published schema
//!
//! ## Does not own
//! - The runtime `scores` payload assertions (`read_scores_tests`)
//! - The remaining published floor text (`read_floors_tests`) and the runtime
//!   binding of the provenance floor (`read_retrieve_provenance_tests`)
//! - The published rendering and channel text (`schema_read_defs_exact_tests`
//!   and `schema_read_props_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the published read schema text.

use serde_json::{Value, json};

/// One published `retrieve` property, located by its published key.
///
/// The property is cloned so a caller can assert on its JSON without borrowing
/// the tool definition the whole table is walked out of.
fn published_prop(name: &str) -> Value {
    crate::schema_read_defs::read_defs()
        .into_iter()
        .find(|def| def.name == "retrieve")
        .expect("retrieve tool definition")
        .props
        .into_iter()
        .find(|(key, _)| *key == name)
        .map_or_else(|| panic!("`{name}` published"), |(_, schema)| schema)
}

/// The published schema is the contract a caller reads before choosing a
/// floor, so the `retrieve` description states what `scores.age_factor` is
/// (the applied retention factor, its range, and that it is never null for a
/// returned hit) and the `final` floor states that it applies to the demoted
/// value, which makes a floor's exclusions predictable from the reported
/// scores.
#[test]
fn the_published_schema_documents_the_age_factor_and_the_demoted_final_floor() {
    let tool = crate::schema_read_defs::read_defs()
        .into_iter()
        .find(|def| def.name == "retrieve")
        .expect("retrieve tool definition");
    let description = tool.description;
    assert!(
        description.contains("`scores.age_factor`"),
        "the description names the field: {description}"
    );
    assert!(
        description.contains("retention factor applied to `scores.final`"),
        "the description states the applied retention factor: {description}"
    );
    assert!(
        description.contains("`[retention_floor, 1]`"),
        "the description states the range: {description}"
    );
    assert!(
        description.contains("never null for a returned hit"),
        "the description states the null contract: {description}"
    );
    let doc = published_prop("min_scores").to_string();
    assert!(
        doc.contains("demoted value"),
        "the final floor states the demoted value: {doc}"
    );
    assert!(
        doc.contains("scores.age_factor"),
        "the final floor names the demoting factor: {doc}"
    );
}

/// The final floor filters a value two multipliers have already lowered, so
/// the published description names `scores.age_factor` first and then
/// `scores.trust_factor`, in application order; a floor that named only one
/// demoter would let a caller predict a floor's exclusions from the wrong
/// value.
#[test]
fn the_published_final_floor_names_both_demoting_multipliers_in_order() {
    let doc = published_prop("min_scores").to_string();
    for (claim, needle) in [
        (
            "the floor applies to the demoted value",
            "The floor applies to the demoted value",
        ),
        (
            "the age curve demotes the fused score first",
            "the fused score after `scores.age_factor`",
        ),
        (
            "the trust multiplier demotes it second",
            "and then `scores.trust_factor` have lowered it",
        ),
        (
            "the two demoters are pinned to their application order",
            "in that order",
        ),
        (
            "the exclusions stay predictable from the reported final",
            "predict what a floor excludes from the reported `scores.final`",
        ),
    ] {
        assert!(doc.contains(needle), "the schema states {claim}: {doc}");
    }
}

/// The published schema is the contract a caller reads before interpreting a
/// hit, so the `retrieve` description states what `scores.trust_factor` is
/// (the applied trust multiplier, its range, and that it is always present),
/// that it only reorders hits, and that it is not a confidence value.
#[test]
fn the_published_schema_documents_the_trust_factor() {
    let tool = crate::schema_read_defs::read_defs()
        .into_iter()
        .find(|def| def.name == "retrieve")
        .expect("retrieve tool definition");
    let description = tool.description;
    for (claim, needle) in [
        ("the description names the field", "`scores.trust_factor`"),
        (
            "the description states the applied trust multiplier",
            "trust multiplier applied to `scores.final` at the shaping site",
        ),
        (
            "the description states the range",
            "`[the smallest configured trust ratio, 1.0]`",
        ),
        (
            "the description states it is always present",
            "always present",
        ),
        (
            "the description limits the effect to ordering",
            "only reorders hits",
        ),
        (
            "the description pins the fields it never changes",
            "never changes `category`, `epistemic_kind`, or `confidence`",
        ),
        (
            "the description says it is not confidence",
            "not a confidence value",
        ),
    ] {
        assert!(
            description.contains(needle),
            "the schema states {claim}: {description}"
        );
    }
}

/// A caller picks a rung from the published schema alone, so the description
/// states that the weakest kept rung decides the answer set, and the property
/// is optional because its name is absent from `required`, which is the
/// machine-readable statement that omitting it is valid.
#[test]
fn the_published_schema_documents_the_provenance_floor_rungs() {
    let floor = published_prop("provenance_floor");
    assert_eq!(floor["type"], json!("string"), "{floor}");
    assert_eq!(
        floor["enum"],
        json!(["verified", "stated", "third_party", "inferred"]),
        "the four rungs are the whole ladder: {floor}"
    );
    let doc = floor.to_string();
    assert!(
        doc.contains("Weakest provenance rung kept"),
        "the schema states the weakest kept rung is the contract: {doc}"
    );
    let tool = crate::schema_read_defs::read_defs()
        .into_iter()
        .find(|def| def.name == "retrieve")
        .expect("retrieve tool definition");
    assert!(
        !tool.required.contains(&"provenance_floor"),
        "the floor is optional, so its name stays out of required: {:?}",
        tool.required
    );
}

/// The floor is a read filter, so the published promise is that a hit below it
/// leaves the answer set instead of moving lower in the ranking.
#[test]
fn the_published_provenance_floor_promises_membership_and_nothing_else() {
    let doc = published_prop("provenance_floor").to_string();
    for (claim, needle) in [
        (
            "a hit below the floor is removed",
            "hits below it are removed",
        ),
        ("removal replaces a lower rank", "removed, not ranked lower"),
    ] {
        assert!(doc.contains(needle), "the schema states {claim}: {doc}");
    }
}

/// The property is optional, so the published description states what an
/// omitted value means: the bank's own default decides the floor.
#[test]
fn the_published_provenance_floor_states_the_omitted_default() {
    let doc = published_prop("provenance_floor").to_string();
    for (claim, needle) in [(
        "omitting the floor uses the bank default",
        "Omitted uses the bank default",
    )] {
        assert!(doc.contains(needle), "the schema states {claim}: {doc}");
    }
}
