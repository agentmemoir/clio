// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of tool-group query behaviour: guidance sentences, closures, custom selections, names, and indexes.
//!
//! ## Owns
//! - Guidance sentence, closure, and custom-selection behaviour
//! - Name, preset, and index round-trips
//!
//! ## Does not own
//! - Approved membership, counts, and preset exactness (`mcp_tool_groups_contract_tests`)
//! - Tool dispatch, listing, or schema shapes
//!
//! ## Boundary
//!
//! Changes to query or closure behaviour should normally update tests here.
//! Changes to approved membership should not require changes here.
//!
//! Keep this module focused on tool-group behaviour.

use super::*;
use std::collections::BTreeSet;

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
    assert_eq!(ToolPreset::ALL.len(), 4);
    for preset in ToolPreset::ALL {
        assert_ne!(preset.as_str(), "");
        assert_eq!(ToolPreset::from_str(preset.as_str()), Some(preset));
    }
    assert_eq!(ToolPreset::from_str("nope"), None);
}

#[test]
fn every_group_has_one_row_and_an_exact_index() {
    assert_eq!(GROUP_TABLE.len(), ToolGroup::ALL.len());
    for (position, group) in ToolGroup::ALL.iter().enumerate() {
        assert_eq!(group.index(), position, "group {}", group.as_str());
        let rows = GROUP_TABLE.iter().filter(|row| row.group == *group).count();
        assert_eq!(rows, 1, "group {}", group.as_str());
    }
    let unique: BTreeSet<ToolGroup> = ToolGroup::ALL.into_iter().collect();
    assert_eq!(unique.len(), ToolGroup::ALL.len());
}
