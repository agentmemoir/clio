// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the documentation metadata of the triple, belief, graph,
//! and associations records: that every documented flag carries a description
//! and a valid value kind.
//!
//! ## Owns
//! - The `check_metadata` contract over this tranche's eight records
//! - The recorded `--epistemic-kind` code default of `triple add`
//! - The tranche's flag metadata: the enum lists the consumers accept, the
//!   value kinds their parsers use, and the recorded defaults
//!
//! ## Does not own
//! - The usage text and parser sets (`cli_command_registry_graph_records_tests`)
//! - The metadata check itself (`cli_command_registry`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on this tranche's flag metadata.

use crate::cli_command_registry::check_metadata;
use crate::cli_command_registry_test_support::record;
use crate::cli_command_spec::ValueKind;

/// Every documented flag in this tranche carries a description and, for a value
/// flag, a placeholder and a non-empty kind.
#[test]
fn the_graph_records_flag_metadata_is_complete() {
    let specs: Vec<&crate::cli_command_spec::CommandSpec> = super::GRAPH_RECORDS.iter().collect();
    assert_eq!(check_metadata(&specs), Ok(()));
}

/// `triple add` records its `--epistemic-kind` code default, the value
/// `triple_add_call` falls back to when the flag is absent.
#[test]
fn triple_add_records_the_epistemic_kind_default() {
    let spec = record("triple add");
    let flag = spec
        .documented
        .iter()
        .find(|flag| flag.long == "epistemic-kind")
        .expect("epistemic-kind is documented");
    assert_eq!(flag.default, Some("fact"));
}

/// Every `--update-rule` flag in this tranche accepts only `discrete`; the
/// `update` verb is the one that also accepts `continuous`.
#[test]
fn the_graph_records_update_rule_accepts_only_discrete() {
    let mut seen = false;
    for spec in super::GRAPH_RECORDS {
        for flag in spec.documented {
            if flag.long == "update-rule" {
                seen = true;
                assert_eq!(
                    flag.value.expect("a value flag").kind,
                    ValueKind::Enum(&["discrete"]),
                    "`{} --update-rule`",
                    spec.name,
                );
            }
        }
    }
    assert!(seen, "the tranche documents `--update-rule`");
}

/// The value kind one record's documented flag carries, and its recorded
/// default.
fn value(name: &str, flag: &str) -> (ValueKind, Option<&'static str>) {
    let documented = record(name)
        .documented
        .iter()
        .find(|candidate| candidate.long == flag)
        .unwrap_or_else(|| panic!("`{name}` documents `--{flag}`"));
    (
        documented
            .value
            .unwrap_or_else(|| panic!("`{name} --{flag}` takes a value"))
            .kind,
        documented.default,
    )
}

/// Every documented flag carries the kind its consumer parses: the enum
/// lists name exactly the accepted values (`cli_read_belief`'s two axes,
/// `cli_write_graph`'s two epistemic kinds and supersede labels, the
/// `clio_types` provenance wire table, and `cli_read_graph`'s two edge
/// types), the counted flags are `Uint`, the weights and confidences are
/// `Float`, and every remaining flag stays free text or the shared `--output` global.
#[test]
fn the_graph_records_flag_kinds_match_their_consumers() {
    for spec in super::GRAPH_RECORDS {
        for flag in spec.documented {
            let Some(value) = flag.value else { continue };
            let expected = match (spec.name, flag.long) {
                ("triple query", "time-axis") => ValueKind::Enum(&["valid", "transaction"]),
                ("triple add", "epistemic-kind") => ValueKind::Enum(&["fact", "belief"]),
                ("triple add", "source-type") | ("belief observe", "source") => {
                    ValueKind::Enum(clio_types::SourceType::WIRE)
                }
                ("triple add", "supersede") => ValueKind::Enum(&["true", "false"]),
                ("triple add" | "triple end", "update-rule") => ValueKind::Enum(&["discrete"]),
                ("graph query", "edge-type") => ValueKind::Enum(&["coactivation", "explicit"]),
                ("graph query", "max-hops") => ValueKind::Uint,
                ("graph query" | "associations", "min-weight")
                | ("graph link", "weight")
                | ("triple add" | "belief observe", "confidence") => ValueKind::Float,
                (_, "output") => ValueKind::Enum(&["json", "text"]),
                _ => ValueKind::Text,
            };
            assert_eq!(value.kind, expected, "`{} --{}`", spec.name, flag.long);
        }
    }
}

/// The recorded defaults are the values the tools apply when the flags are
/// omitted: `triple query` walks the valid axis, `triple add` supersedes,
/// and `clio-retrieve`'s association engine weights new edges at 1.0, with
/// no named constants for any of the three.
#[test]
fn the_recorded_defaults_are_the_values_the_tools_apply() {
    for (name, flag, default) in [
        ("triple query", "time-axis", Some("valid")),
        ("triple add", "supersede", Some("true")),
        ("graph link", "weight", Some("1.0")),
    ] {
        assert_eq!(value(name, flag).1, default, "`{name} --{flag}` default");
    }
}
