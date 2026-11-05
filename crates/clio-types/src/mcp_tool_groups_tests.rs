// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the MCP tool-group catalog, its
//! guidance sentences, and its preset and closure derivations.
//!
//! ## Owns
//! - Membership, guidance, preset, closure, and custom-selection contracts
//! - Mutation proofs that each contract check rejects a broken catalog
//!
//! ## Does not own
//! - Tool dispatch, listing, or schema shapes
//!
//! ## Boundary
//!
//! Changes to group membership or presets should normally update tests here.
//! Changes to tool behaviour should not require changes here.
//!
//! Keep this module focused on the tool-group catalog contract.

use super::*;
use crate::mcp_tools::bound_tools;
use std::collections::BTreeSet;

const RETRIEVAL_WITHOUT_RETRIEVE: &[&str] = &["intent_gate", "compose_context", "associations"];
const GRAPH_WITH_RETRIEVE: &[&str] = &["graph_link", "graph_query", "retrieve"];
const GRAPH_WITH_UNKNOWN: &[&str] = &["graph_link", "graph_query", "not_a_tool"];

fn table_rows() -> Vec<(ToolGroup, &'static [&'static str])> {
    GROUP_TABLE
        .iter()
        .map(|row| (row.group, row.tools))
        .collect()
}

fn rows_with(
    replacements: &[(ToolGroup, &'static [&'static str])],
) -> Vec<(ToolGroup, &'static [&'static str])> {
    GROUP_TABLE
        .iter()
        .map(|row| {
            let members = replacements
                .iter()
                .find(|(group, _)| *group == row.group)
                .map_or(row.tools, |(_, members)| *members);
            (row.group, members)
        })
        .collect()
}

fn check_membership(rows: &[(ToolGroup, &'static [&'static str])]) -> Result<(), String> {
    let catalog = bound_tools();
    if rows.len() != ToolGroup::ALL.len() {
        return Err(format!(
            "group table has {} rows, expected {}",
            rows.len(),
            ToolGroup::ALL.len()
        ));
    }
    let mut seen: Vec<&'static str> = Vec::new();
    for (group, members) in rows {
        for member in members.iter().copied() {
            if !catalog.contains(&member) {
                return Err(format!("unknown tool `{member}`"));
            }
            if seen.contains(&member) {
                return Err(format!("duplicate membership for `{member}`"));
            }
            if group_of(member) != Some(*group) {
                return Err(format!(
                    "`{member}` is listed under `{}`, not its catalog group",
                    group.as_str()
                ));
            }
            seen.push(member);
        }
    }
    for bound in &catalog {
        if !seen.contains(bound) {
            return Err(format!("catalog tool `{bound}` has no group"));
        }
    }
    if seen.len() != catalog.len() {
        return Err(format!(
            "grouped {} tools, expected {}",
            seen.len(),
            catalog.len()
        ));
    }
    Ok(())
}

fn check_preset(preset: ToolPreset, observed: &[ToolGroup]) -> Result<(), String> {
    let expected = match preset {
        ToolPreset::Minimal => vec![ToolGroup::Retrieval, ToolGroup::Graph],
        ToolPreset::Coding => vec![
            ToolGroup::Retrieval,
            ToolGroup::ItemRead,
            ToolGroup::Persona,
        ],
        ToolPreset::Ops => vec![
            ToolGroup::ItemRead,
            ToolGroup::Ops,
            ToolGroup::Hygiene,
            ToolGroup::Retention,
        ],
        ToolPreset::All => ToolGroup::ALL.to_vec(),
    };
    if observed != expected.as_slice() {
        return Err(format!(
            "preset `{}` is {observed:?}, expected {expected:?}",
            preset.as_str()
        ));
    }
    Ok(())
}

#[test]
fn catalog_groups_every_bound_tool_exactly_once() {
    let rows = table_rows();
    check_membership(&rows).expect("production group table must pass");
    let grouped: BTreeSet<&str> = rows
        .iter()
        .flat_map(|(_, members)| members.iter().copied())
        .collect();
    let bound: BTreeSet<&str> = bound_tools().into_iter().collect();
    assert_eq!(grouped, bound);
    assert_eq!(grouped.len(), 86);
    assert_eq!(rows.len(), 17);
    assert_eq!(group_of("retrieve"), Some(ToolGroup::Retrieval));
    assert_eq!(group_of("erase_request"), Some(ToolGroup::Compliance));
    assert_eq!(group_of("search"), None);
}

#[test]
fn membership_checker_rejects_unknown_tool() {
    let rows = rows_with(&[(ToolGroup::Graph, GRAPH_WITH_UNKNOWN)]);
    let error = check_membership(&rows).expect_err("an unknown tool must fail");
    assert!(error.contains("not_a_tool"), "message was: {error}");
}

#[test]
fn membership_checker_rejects_ungrouped_tool() {
    let rows = rows_with(&[(ToolGroup::Retrieval, RETRIEVAL_WITHOUT_RETRIEVE)]);
    let error = check_membership(&rows).expect_err("an ungrouped tool must fail");
    assert!(error.contains("retrieve"), "message was: {error}");
}

#[test]
fn membership_checker_rejects_duplicate_membership() {
    let rows = rows_with(&[(ToolGroup::Graph, GRAPH_WITH_RETRIEVE)]);
    let error = check_membership(&rows).expect_err("a duplicate tool must fail");
    assert!(error.contains("retrieve"), "message was: {error}");
}

#[test]
fn membership_checker_rejects_wrong_row_count() {
    let rows = table_rows();
    let error = check_membership(&rows[..16]).expect_err("a short table must fail");
    assert!(error.contains("17"), "message was: {error}");
}

#[test]
fn membership_checker_detects_a_moved_tool() {
    let rows = rows_with(&[
        (ToolGroup::Retrieval, RETRIEVAL_WITHOUT_RETRIEVE),
        (ToolGroup::Graph, GRAPH_WITH_RETRIEVE),
    ]);
    let error = check_membership(&rows).expect_err("a moved tool must fail");
    assert!(error.contains("retrieve"), "message was: {error}");
}

#[test]
fn when_to_use_matches_the_approved_sentences() {
    let expected = [
        (
            ToolGroup::Retrieval,
            "Find memory or build grounded context.",
        ),
        (
            ToolGroup::ItemWrite,
            "Create, correct or remove memory items.",
        ),
        (
            ToolGroup::ItemRead,
            "Read, inspect or summarize stored items.",
        ),
        (ToolGroup::Persona, "Read or record stable preferences."),
        (ToolGroup::Task, "Record task state and failure history."),
        (ToolGroup::Memtree, "Read hierarchical or temporal memory."),
        (ToolGroup::Triple, "Record or query factual triples."),
        (ToolGroup::Belief, "Record or inspect beliefs."),
        (ToolGroup::Graph, "Read or add memory relationships."),
        (ToolGroup::Hygiene, "Inspect or clean memory hygiene."),
        (
            ToolGroup::Portability,
            "Move or review memory exports and imports.",
        ),
        (ToolGroup::Ops, "Diagnose, verify or repair operations."),
        (ToolGroup::Retention, "Read or set retention policy."),
        (ToolGroup::Config, "Read or change configuration."),
        (ToolGroup::Ranking, "Read or change ranking settings."),
        (ToolGroup::Sync, "Exchange memory between peers."),
        (ToolGroup::Compliance, "Track subject requests and erasure."),
    ];
    assert_eq!(expected.len(), 17);
    for (group, sentence) in expected {
        let observed = group.spec().when_to_use;
        assert_eq!(observed, sentence, "group {}", group.as_str());
        assert!(!observed.is_empty(), "group {}", group.as_str());
        assert!(observed.ends_with('.'), "group {}", group.as_str());
    }
}

#[test]
fn presets_match_their_declared_sets() {
    for preset in [
        ToolPreset::Minimal,
        ToolPreset::Coding,
        ToolPreset::Ops,
        ToolPreset::All,
    ] {
        check_preset(preset, preset.groups()).expect("declared preset set must pass");
    }
    let mut coding_plus_item_write = ToolPreset::Coding.groups().to_vec();
    coding_plus_item_write.push(ToolGroup::ItemWrite);
    assert!(check_preset(ToolPreset::Coding, &coding_plus_item_write).is_err());
}

#[test]
fn presets_derive_membership_from_groups() {
    for preset in [
        ToolPreset::Minimal,
        ToolPreset::Coding,
        ToolPreset::Ops,
        ToolPreset::All,
    ] {
        let resolved = preset.resolve();
        let unique: BTreeSet<ToolGroup> = resolved.iter().copied().collect();
        assert_eq!(
            unique.len(),
            resolved.len(),
            "preset {} resolved duplicates",
            preset.as_str()
        );
        let declared: BTreeSet<&str> = preset
            .groups()
            .iter()
            .flat_map(|group| group.spec().tools.iter().copied())
            .collect();
        let derived: BTreeSet<&str> = resolved
            .iter()
            .flat_map(|group| group.spec().tools.iter().copied())
            .collect();
        assert_eq!(derived, declared, "preset {}", preset.as_str());
        assert_eq!(
            resolved,
            closure(preset.groups()),
            "preset {}",
            preset.as_str()
        );
    }
}

#[test]
fn closure_adds_requirements_in_canonical_order() {
    assert_eq!(
        closure(&[ToolGroup::Sync]),
        vec![ToolGroup::ItemWrite, ToolGroup::ItemRead, ToolGroup::Sync]
    );
    assert_eq!(
        closure(&[ToolGroup::Portability]),
        vec![
            ToolGroup::ItemWrite,
            ToolGroup::ItemRead,
            ToolGroup::Portability
        ]
    );
    assert_eq!(
        closure(&[ToolGroup::Compliance]),
        vec![
            ToolGroup::ItemWrite,
            ToolGroup::ItemRead,
            ToolGroup::Compliance
        ]
    );
    assert_eq!(
        closure(&[ToolGroup::Hygiene]),
        vec![ToolGroup::ItemRead, ToolGroup::Hygiene]
    );
    assert_eq!(
        closure(&[ToolGroup::Ops]),
        vec![ToolGroup::ItemRead, ToolGroup::Ops]
    );
    assert_eq!(
        closure(&[ToolGroup::Retention]),
        vec![ToolGroup::ItemRead, ToolGroup::Retention]
    );
    assert_eq!(
        closure(&[ToolGroup::ItemWrite]),
        vec![ToolGroup::ItemWrite, ToolGroup::ItemRead]
    );
    assert_eq!(closure(&[ToolGroup::Retrieval]), vec![ToolGroup::Retrieval]);
    let everything = closure(&ToolGroup::ALL);
    assert_eq!(everything, ToolGroup::ALL.to_vec());
    let unique: BTreeSet<ToolGroup> = everything.iter().copied().collect();
    assert_eq!(unique.len(), everything.len());
}

#[test]
fn custom_starts_from_minimal_and_adds_closure() {
    assert_eq!(
        custom(&[]).expect("the empty selection is valid"),
        ToolPreset::Minimal.resolve()
    );
    assert_eq!(
        custom(&["sync"]).expect("sync is a group"),
        vec![
            ToolGroup::Retrieval,
            ToolGroup::ItemWrite,
            ToolGroup::ItemRead,
            ToolGroup::Graph,
            ToolGroup::Sync
        ]
    );
    let single = custom(&["sync"]).expect("sync is a group");
    let repeated = custom(&["sync", "sync"]).expect("a repeated group is valid");
    assert_eq!(repeated, single);
    let unique: BTreeSet<ToolGroup> = repeated.iter().copied().collect();
    assert_eq!(unique.len(), repeated.len());
}

#[test]
fn custom_rejects_unknown_names_without_partial_result() {
    let error = custom(&["nope"]).expect_err("an unknown group must fail");
    assert_eq!(
        error,
        ToolGroupError::UnknownGroup {
            name: "nope".to_string()
        }
    );
    assert_eq!(error.to_string(), "unknown tool group `nope`");
    let as_trait_object: &dyn std::error::Error = &error;
    assert!(as_trait_object.to_string().contains("nope"));

    let mixed = custom(&["sync", "nope"]).expect_err("a mixed selection must fail");
    assert_eq!(
        mixed,
        ToolGroupError::UnknownGroup {
            name: "nope".to_string()
        }
    );

    let preset_name = custom(&["all"]).expect_err("a preset name is not a group");
    assert_eq!(
        preset_name,
        ToolGroupError::UnknownGroup {
            name: "all".to_string()
        }
    );
}

#[test]
fn names_round_trip_and_default_is_minimal() {
    assert_eq!(DEFAULT_PRESET, ToolPreset::Minimal);
    for group in ToolGroup::ALL {
        assert_ne!(group.as_str(), "");
        assert_eq!(ToolGroup::from_str(group.as_str()), Some(group));
    }
    assert_eq!(ToolGroup::from_str("nope"), None);
    for preset in [
        ToolPreset::Minimal,
        ToolPreset::Coding,
        ToolPreset::Ops,
        ToolPreset::All,
    ] {
        assert_ne!(preset.as_str(), "");
        assert_eq!(ToolPreset::from_str(preset.as_str()), Some(preset));
    }
    assert_eq!(ToolPreset::from_str("nope"), None);
}
