// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for contract tests that pin the approved tool-group data and preset sets to an independent fixture.
//!
//! ## Owns
//! - The 86-tool name-to-group fixture, written independently of catalog order
//! - Exact table, row-order, per-group count, and preset checks
//! - Mutation proofs that each contract check rejects a broken catalog
//!
//! ## Does not own
//! - Closure, custom selection, or name parsing behaviour (`mcp_tool_groups_tests`)
//!
//! ## Boundary
//!
//! Changes to approved membership or preset sets must update the fixture here.
//! Changes to query behaviour should not require changes here.
//!
//! Keep this module focused on the approved contract.

use super::*;
use crate::mcp_tools::bound_tools;
use std::collections::{BTreeMap, BTreeSet};

const APPROVED_TOOL_GROUPS: [(&str, ToolGroup); 86] = [
    ("admit_preview", ToolGroup::ItemWrite),
    ("admit_preview_batch", ToolGroup::ItemWrite),
    ("associations", ToolGroup::Retrieval),
    ("audit_trail", ToolGroup::ItemRead),
    ("batch", ToolGroup::ItemWrite),
    ("belief_history", ToolGroup::Belief),
    ("belief_observe", ToolGroup::Belief),
    ("canonical_get", ToolGroup::ItemRead),
    ("canonical_put", ToolGroup::ItemWrite),
    ("compose_context", ToolGroup::Retrieval),
    ("config_get", ToolGroup::Config),
    ("config_profile_apply", ToolGroup::Config),
    ("config_profiles", ToolGroup::Config),
    ("config_set", ToolGroup::Config),
    ("consolidate", ToolGroup::ItemRead),
    ("correct", ToolGroup::ItemWrite),
    ("diagnose", ToolGroup::Ops),
    ("discard", ToolGroup::ItemWrite),
    ("doctor", ToolGroup::Ops),
    ("erase_request", ToolGroup::Compliance),
    ("export", ToolGroup::Portability),
    ("failure_record", ToolGroup::Task),
    ("failures_for_task", ToolGroup::Task),
    ("get", ToolGroup::ItemRead),
    ("get_gist", ToolGroup::ItemRead),
    ("get_snapshot", ToolGroup::ItemRead),
    ("graph_link", ToolGroup::Graph),
    ("graph_query", ToolGroup::Graph),
    ("hygiene_audit", ToolGroup::Hygiene),
    ("hygiene_clean", ToolGroup::Hygiene),
    ("hygiene_log_list", ToolGroup::Hygiene),
    ("import", ToolGroup::Portability),
    ("import_provider", ToolGroup::Portability),
    ("inspect", ToolGroup::ItemRead),
    ("intent_gate", ToolGroup::Retrieval),
    ("invalidate", ToolGroup::ItemWrite),
    ("maintenance_status", ToolGroup::ItemRead),
    ("memtree_get", ToolGroup::Memtree),
    ("memtree_query", ToolGroup::Memtree),
    ("persona_get", ToolGroup::Persona),
    ("persona_observe_preference", ToolGroup::Persona),
    ("persona_put_stable", ToolGroup::Persona),
    ("ranking_env_get", ToolGroup::Ranking),
    ("ranking_env_set", ToolGroup::Ranking),
    ("reindex", ToolGroup::Ops),
    ("repair", ToolGroup::Ops),
    ("retention_profile_get", ToolGroup::Retention),
    ("retention_profile_set", ToolGroup::Retention),
    ("retrieve", ToolGroup::Retrieval),
    ("review_export", ToolGroup::Portability),
    ("scratchpad_clear", ToolGroup::ItemWrite),
    ("scratchpad_read", ToolGroup::ItemRead),
    ("scratchpad_write", ToolGroup::ItemWrite),
    ("shared_discard", ToolGroup::ItemWrite),
    ("shared_retrieve", ToolGroup::ItemRead),
    ("shared_store", ToolGroup::ItemWrite),
    ("stats", ToolGroup::Ops),
    ("store", ToolGroup::ItemWrite),
    ("subject_request_close", ToolGroup::Compliance),
    ("subject_request_copy", ToolGroup::Compliance),
    ("subject_request_events", ToolGroup::Compliance),
    ("subject_request_exception", ToolGroup::Compliance),
    ("subject_request_path_complete", ToolGroup::Compliance),
    ("subject_request_peer_disposition", ToolGroup::Compliance),
    ("subject_request_peer_retry", ToolGroup::Compliance),
    ("subject_request_peer_status", ToolGroup::Compliance),
    ("subject_request_retry", ToolGroup::Compliance),
    ("subject_request_start", ToolGroup::Compliance),
    ("subject_request_status", ToolGroup::Compliance),
    ("summarize", ToolGroup::ItemRead),
    ("sync_ack_skip", ToolGroup::Sync),
    ("sync_key_status", ToolGroup::Sync),
    ("sync_pull", ToolGroup::Sync),
    ("sync_push", ToolGroup::Sync),
    ("sync_serve", ToolGroup::Sync),
    ("sync_status", ToolGroup::Sync),
    ("task_get", ToolGroup::Task),
    ("task_history", ToolGroup::Task),
    ("task_upsert", ToolGroup::Task),
    ("temporal_history", ToolGroup::Memtree),
    ("triple_add", ToolGroup::Triple),
    ("triple_end", ToolGroup::Triple),
    ("triple_query", ToolGroup::Triple),
    ("update", ToolGroup::ItemWrite),
    ("validate", ToolGroup::ItemWrite),
    ("verify", ToolGroup::Ops),
];

const APPROVED_GROUP_COUNTS: [(ToolGroup, usize); 17] = [
    (ToolGroup::Retrieval, 4),
    (ToolGroup::ItemWrite, 14),
    (ToolGroup::ItemRead, 11),
    (ToolGroup::Persona, 3),
    (ToolGroup::Task, 5),
    (ToolGroup::Memtree, 3),
    (ToolGroup::Triple, 3),
    (ToolGroup::Belief, 2),
    (ToolGroup::Graph, 2),
    (ToolGroup::Hygiene, 3),
    (ToolGroup::Portability, 4),
    (ToolGroup::Ops, 6),
    (ToolGroup::Retention, 2),
    (ToolGroup::Config, 4),
    (ToolGroup::Ranking, 2),
    (ToolGroup::Sync, 6),
    (ToolGroup::Compliance, 12),
];

const APPROVED_PRESETS: [(ToolPreset, &[ToolGroup]); 4] = [
    (
        ToolPreset::Minimal,
        &[ToolGroup::Retrieval, ToolGroup::Graph],
    ),
    (
        ToolPreset::Coding,
        &[
            ToolGroup::Retrieval,
            ToolGroup::ItemRead,
            ToolGroup::Persona,
        ],
    ),
    (
        ToolPreset::Ops,
        &[
            ToolGroup::ItemRead,
            ToolGroup::Ops,
            ToolGroup::Hygiene,
            ToolGroup::Retention,
        ],
    ),
    (
        ToolPreset::All,
        &[
            ToolGroup::Retrieval,
            ToolGroup::ItemWrite,
            ToolGroup::ItemRead,
            ToolGroup::Persona,
            ToolGroup::Task,
            ToolGroup::Memtree,
            ToolGroup::Triple,
            ToolGroup::Belief,
            ToolGroup::Graph,
            ToolGroup::Hygiene,
            ToolGroup::Portability,
            ToolGroup::Ops,
            ToolGroup::Retention,
            ToolGroup::Config,
            ToolGroup::Ranking,
            ToolGroup::Sync,
            ToolGroup::Compliance,
        ],
    ),
];

fn table_rows() -> Vec<(ToolGroup, &'static [&'static str])> {
    GROUP_TABLE
        .iter()
        .map(|row| (row.group, row.tools))
        .collect()
}

fn rows_with(
    replacements: &[(ToolGroup, &'static [&'static str])],
) -> Vec<(ToolGroup, &'static [&'static str])> {
    let mut rows = table_rows();
    for (group, members) in replacements {
        for row in &mut rows {
            if row.0 == *group {
                row.1 = members;
            }
        }
    }
    rows
}

fn approved_tools() -> BTreeMap<&'static str, ToolGroup> {
    APPROVED_TOOL_GROUPS.into_iter().collect()
}

fn check_table(rows: &[(ToolGroup, &'static [&'static str])]) -> Result<(), String> {
    if rows.len() != ToolGroup::ALL.len() {
        return Err(format!(
            "group table has {} rows, expected {}",
            rows.len(),
            ToolGroup::ALL.len()
        ));
    }
    for (index, (group, _)) in rows.iter().enumerate() {
        let expected = ToolGroup::ALL[index];
        if *group != expected {
            return Err(format!(
                "row {index} is `{}`, expected `{}`",
                group.as_str(),
                expected.as_str()
            ));
        }
    }
    let mut observed: BTreeMap<&'static str, ToolGroup> = BTreeMap::new();
    for (group, members) in rows {
        for member in members.iter().copied() {
            if observed.insert(member, *group).is_some() {
                return Err(format!("tool `{member}` appears in more than one row"));
            }
        }
    }
    let approved = approved_tools();
    for (name, group) in &approved {
        match observed.get(name) {
            None => return Err(format!("approved tool `{name}` has no group")),
            Some(found) if found != group => {
                return Err(format!(
                    "tool `{name}` is in `{}`, expected `{}`",
                    found.as_str(),
                    group.as_str()
                ));
            }
            Some(_) => {}
        }
    }
    for name in observed.keys() {
        if !approved.contains_key(name) {
            return Err(format!("tool `{name}` is not in the approved fixture"));
        }
    }
    for (group, expected) in APPROVED_GROUP_COUNTS {
        let count = rows
            .iter()
            .find(|(row_group, _)| *row_group == group)
            .map_or(0, |(_, members)| members.len());
        if count != expected {
            return Err(format!(
                "group `{}` has {count} tools, expected {expected}",
                group.as_str()
            ));
        }
    }
    Ok(())
}

fn check_preset(
    preset: ToolPreset,
    declared: &[ToolGroup],
    resolved: &[ToolGroup],
) -> Result<(), String> {
    let (_, approved) = APPROVED_PRESETS
        .iter()
        .find(|(name, _)| *name == preset)
        .ok_or_else(|| format!("preset `{}` has no approved set", preset.as_str()))?;
    let observed: BTreeSet<ToolGroup> = declared.iter().copied().collect();
    if observed.len() != declared.len() {
        return Err(format!(
            "preset `{}` declares a duplicate group",
            preset.as_str()
        ));
    }
    let expected: BTreeSet<ToolGroup> = approved.iter().copied().collect();
    if observed != expected {
        return Err(format!(
            "preset `{}` declares {declared:?}, expected {approved:?}",
            preset.as_str()
        ));
    }
    let resolved_groups: BTreeSet<ToolGroup> = resolved.iter().copied().collect();
    if resolved_groups.len() != resolved.len() {
        return Err(format!(
            "preset `{}` resolves a duplicate group",
            preset.as_str()
        ));
    }
    let tools = approved_tools();
    let expected_tools: BTreeSet<&str> = tools
        .iter()
        .filter(|(_, group)| expected.contains(group))
        .map(|(name, _)| *name)
        .collect();
    let resolved_tools: BTreeSet<&str> = tools
        .iter()
        .filter(|(_, group)| resolved_groups.contains(group))
        .map(|(name, _)| *name)
        .collect();
    if resolved_tools != expected_tools {
        let missing: Vec<&str> = expected_tools
            .difference(&resolved_tools)
            .copied()
            .collect();
        let extra: Vec<&str> = resolved_tools
            .difference(&expected_tools)
            .copied()
            .collect();
        return Err(format!(
            "preset `{}` resolves tools missing {missing:?} extra {extra:?}",
            preset.as_str()
        ));
    }
    Ok(())
}

#[test]
fn approved_fixture_covers_every_bound_tool() {
    let approved: BTreeSet<&str> = APPROVED_TOOL_GROUPS.iter().map(|(name, _)| *name).collect();
    let bound: BTreeSet<&str> = bound_tools().into_iter().collect();
    assert_eq!(APPROVED_TOOL_GROUPS.len(), 86);
    assert_eq!(approved, bound);
}

#[test]
fn table_matches_the_approved_fixture() {
    let rows = table_rows();
    check_table(&rows).expect("the approved group table must pass");
    assert_eq!(rows.len(), 17);
}

#[test]
fn checker_rejects_an_unknown_tool() {
    let rows = rows_with(&[(
        ToolGroup::Graph,
        &["graph_link", "graph_query", "not_a_tool"],
    )]);
    let error = check_table(&rows).expect_err("an unknown tool must fail");
    assert!(error.contains("not_a_tool"), "message was: {error}");
}

#[test]
fn checker_rejects_a_missing_tool() {
    let rows = rows_with(&[(
        ToolGroup::Retrieval,
        &["intent_gate", "compose_context", "associations"],
    )]);
    let error = check_table(&rows).expect_err("a missing tool must fail");
    assert!(error.contains("retrieve"), "message was: {error}");
}

#[test]
fn checker_rejects_a_duplicate_tool() {
    let rows = rows_with(&[(ToolGroup::Graph, &["graph_link", "graph_query", "retrieve"])]);
    let error = check_table(&rows).expect_err("a duplicate tool must fail");
    assert!(error.contains("retrieve"), "message was: {error}");
}

#[test]
fn checker_rejects_a_short_table() {
    let rows = table_rows();
    let error = check_table(&rows[..16]).expect_err("a short table must fail");
    assert!(error.contains("17"), "message was: {error}");
}

#[test]
fn checker_detects_a_moved_tool() {
    let rows = rows_with(&[
        (
            ToolGroup::ItemWrite,
            &[
                "admit_preview",
                "admit_preview_batch",
                "update",
                "invalidate",
                "discard",
                "correct",
                "batch",
                "validate",
                "canonical_put",
                "shared_store",
                "shared_discard",
                "scratchpad_write",
                "scratchpad_clear",
            ],
        ),
        (ToolGroup::Graph, &["graph_link", "graph_query", "store"]),
    ]);
    let error = check_table(&rows).expect_err("a moved tool must fail");
    assert!(error.contains("store"), "message was: {error}");
}

#[test]
fn presets_match_the_approved_fixture() {
    for (preset, _) in APPROVED_PRESETS {
        check_preset(preset, preset.groups(), &preset.resolve())
            .expect("the approved preset must pass");
    }
}

/// A preset's tool list is exactly the catalog rows of its resolved groups, and
/// the default preset therefore names only its own groups' tools.
#[test]
fn preset_tools_match_the_resolved_group_rows() {
    for (preset, _) in APPROVED_PRESETS {
        let mut expected: Vec<&'static str> = GROUP_TABLE
            .iter()
            .filter(|row| preset.resolve().contains(&row.group))
            .flat_map(|row| row.tools.iter().copied())
            .collect();
        let tools = preset.tools();
        assert_eq!(tools, expected, "preset `{}`", preset.as_str());
        let mut sorted = tools.clone();
        sorted.sort_unstable();
        expected.sort_unstable();
        assert_eq!(sorted, expected);
    }
    assert!(DEFAULT_PRESET.tools().contains(&"retrieve"));
    assert!(!DEFAULT_PRESET.tools().contains(&"store"));
}

#[test]
fn preset_checker_rejects_an_extra_group() {
    let mut declared = ToolPreset::Coding.groups().to_vec();
    declared.push(ToolGroup::ItemWrite);
    let error = check_preset(ToolPreset::Coding, &declared, &ToolPreset::Coding.resolve())
        .expect_err("an extra preset group must fail");
    assert!(error.contains("coding"), "message was: {error}");
}
