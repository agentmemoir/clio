// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests covering the config/ranking tool
//! schema text: the published ranking-environment groups, including the
//! retention shaping group and its `half_life_days` knob, the
//! maintenance-trigger group and its inclusive/zero-disables threshold
//! semantics, and the corrected `ranking_env_set` and `config_profile_apply`
//! sentences (process-exit discard, governs-next-retrieval,
//! push-to-retrieval).
//!
//! ## Owns
//! - `ranking_env_get` / `ranking_env_set` description assertions
//! - The `config_profile_apply` description assertions
//! - The published-pack assertion for the retention group text
//!
//! ## Does not own
//! - Schema assembly (see `schema_defs`) or config behavior (see `config_tools`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the config/ranking tool schema text.

use super::*;
use crate::schema::{schema_pack, validate_tool_schema};

fn def(name: &str) -> ToolDef {
    config_defs()
        .into_iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("{name} missing"))
}

#[test]
fn ranking_env_get_description_names_the_retention_group_and_knob() {
    let description = def("ranking_env_get").description;
    assert!(description.contains("retention"), "{description}");
    assert!(description.contains("half_life_days"), "{description}");
    assert!(description.contains("30"), "{description}");
}

#[test]
fn ranking_env_set_patch_description_names_the_retention_group() {
    let patch = &def("ranking_env_set").to_tool()["inputSchema"]["properties"]["patch"];
    assert!(
        patch["description"]
            .as_str()
            .is_some_and(|d| d.contains("retention")),
        "{patch}"
    );
}

/// The maintenance-trigger knobs an operator must see named, with the
/// defaults that differ across the three thresholds (1) and the other three
/// knobs (300, 12, 16).
const MAINTENANCE_TRIGGER_KNOBS: [&str; 6] = [
    "dirty_ancestor_threshold",
    "eligible_hub_threshold",
    "stale_unit_threshold",
    "cooldown_seconds",
    "max_waves_per_hour",
    "hub_budget_per_wave",
];

/// The published `ranking_env_get` sentence names the maintenance-trigger
/// group with every knob and default, and states the inclusive threshold
/// comparison and the zero-disables rule so an operator does not have to
/// guess them.
#[test]
fn ranking_env_get_description_names_the_maintenance_trigger_group() {
    let description = def("ranking_env_get").description;
    assert!(
        description.contains("maintenance_triggers"),
        "{description}"
    );
    for knob in MAINTENANCE_TRIGGER_KNOBS {
        assert!(description.contains(knob), "{description}");
    }
    assert!(description.contains("each default 1"), "{description}");
    assert!(description.contains("300"), "{description}");
    assert!(description.contains("12"), "{description}");
    assert!(description.contains("16"), "{description}");
    assert!(
        description.contains("greater than or equal to the threshold"),
        "{description}"
    );
    assert!(description.contains("`0` disabling"), "{description}");
}

/// The published `ranking_env_set` patch sentence names the
/// maintenance-trigger group with every knob and default, and states the
/// inclusive threshold comparison and the zero-disables rule.
#[test]
fn ranking_env_set_patch_description_names_the_maintenance_trigger_group() {
    let patch = &def("ranking_env_set").to_tool()["inputSchema"]["properties"]["patch"];
    let description = patch["description"].as_str().expect("patch description");
    assert!(
        description.contains("maintenance_triggers"),
        "{description}"
    );
    for knob in MAINTENANCE_TRIGGER_KNOBS {
        assert!(description.contains(knob), "{description}");
    }
    assert!(description.contains("each default 1"), "{description}");
    assert!(description.contains("300"), "{description}");
    assert!(description.contains("12"), "{description}");
    assert!(description.contains("16"), "{description}");
    assert!(
        description.contains("greater than or equal to the threshold"),
        "{description}"
    );
    assert!(description.contains("`0` disables"), "{description}");
}

/// Both published maintenance-trigger sentences state the `0` behaviour of
/// all six knobs: the three thresholds disable their one condition, the
/// cooldown means no cooldown, and the two rate knobs reject `0`.
#[test]
fn published_maintenance_knob_zero_semantics_are_stated() {
    let get = def("ranking_env_get").description;
    let patch = &def("ranking_env_set").to_tool()["inputSchema"]["properties"]["patch"];
    let set = patch["description"].as_str().expect("patch description");
    for description in [get, set] {
        assert!(
            description.contains("`0` disabling") || description.contains("`0` disables"),
            "{description}"
        );
        assert!(description.contains("no cooldown"), "{description}");
        assert!(
            description.contains("`max_waves_per_hour`"),
            "{description}"
        );
        assert!(
            description.contains("`hub_budget_per_wave`"),
            "{description}"
        );
        let waves = description
            .split("`max_waves_per_hour`")
            .nth(1)
            .expect("the max_waves_per_hour clause");
        assert!(
            waves.contains("rejects `0`"),
            "max_waves_per_hour must reject 0: {description}"
        );
        let budget = description
            .split("`hub_budget_per_wave`")
            .nth(1)
            .expect("the hub_budget_per_wave clause");
        assert!(
            budget.contains("rejects `0`"),
            "hub_budget_per_wave must reject 0: {description}"
        );
    }
}

/// The published `ranking_env_set` sentence keeps both true halves: the
/// process-exit discard and the governs-next-retrieval push, with the `trust`
/// group named so a caller knows trust ratios are patchable, and the
/// admission and intent-gate half staying on the process-open environment.
#[test]
fn ranking_env_set_description_states_the_patch_governs_the_next_retrieval() {
    let description = def("ranking_env_set").description;
    assert!(
        description.contains("discarded at process exit"),
        "{description}"
    );
    assert!(
        description.contains("governs the next retrieval"),
        "{description}"
    );
    assert!(description.contains("trust"), "{description}");
    assert!(
        description.contains(
            "the admission policy, the intent gate, and the hygiene weights stay on the ranking environment fixed at process open"
        ),
        "{description}"
    );
}

/// The published `ranking_env_set` sentence names the two patch groups that do
/// not reach the engine a caller would expect: `hygiene` stays on the
/// process-open environment, and `coactivation` reaches the hub but not the
/// retriever's own expansion. Both are accepted and read back, so the
/// description has to say so.
#[test]
fn ranking_env_set_description_names_the_fixed_patch_groups() {
    let description = def("ranking_env_set").description;
    assert!(
        description
            .contains("the hygiene weights stay on the ranking environment fixed at process open"),
        "{description}"
    );
    assert!(
        description.contains(
            "`coactivation` patch reaches the association hub but not the retriever's own query expansion"
        ),
        "{description}"
    );
}

/// The published `config_profile_apply` sentence keeps both true halves: the
/// push-to-retrieval half (a successful apply retunes the already-open
/// retrieval snapshot) and the admission half (admission and the intent gate
/// stay on the process-open environment), with the process-exit discard.
#[test]
fn config_profile_apply_description_states_the_push_to_the_live_retrieval() {
    let description = def("config_profile_apply").description;
    assert!(
        description.contains("pushes the profile's ranking environment"),
        "{description}"
    );
    assert!(description.contains("next retrieval"), "{description}");
    assert!(
        description.contains("discarded at process exit"),
        "{description}"
    );
    assert!(
        description.contains(
            "the admission policy and the intent gate stay on the ranking environment fixed at process open"
        ),
        "{description}"
    );
}

#[test]
fn published_pack_carries_the_retention_knob_text() {
    let pack = schema_pack();
    let tool = pack["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"] == serde_json::json!("ranking_env_get"))
        .unwrap_or_else(|| panic!("ranking_env_get published"))
        .clone();
    let description = tool["description"].as_str().expect("description");
    assert!(description.contains("half_life_days"), "{description}");
    assert_eq!(tool["inputSchema"]["type"], "object");
    let defects = validate_tool_schema(&def("ranking_env_get").to_tool());
    assert!(defects.is_empty(), "{defects:?}");
}
