// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests covering the hygiene tool schema
//! definitions: the `hygiene_clean` action enum and the description that
//! documents it.
//!
//! ## Owns
//! - The `hygiene_clean` action enum and description assertions
//!
//! ## Does not own
//! - Hygiene tool dispatch or handler semantics (see `hygiene_tools`)
//! - Core or additive tool definitions (see `schema_defs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the hygiene tool schema definitions.

use super::*;

/// One hygiene tool definition by name.
fn hygiene_def(name: &str) -> ToolDef {
    hygiene_defs()
        .into_iter()
        .find(|def| def.name == name)
        .unwrap_or_else(|| panic!("{name} missing from the hygiene schema pack"))
}

/// The enum values of one property of one tool definition.
fn enum_values(def: &ToolDef, prop_name: &str) -> Vec<String> {
    def.props
        .iter()
        .find(|(name, _)| *name == prop_name)
        .unwrap_or_else(|| panic!("`{}` has no `{prop_name}` property", def.name))
        .1["enum"]
        .as_array()
        .expect("enum array")
        .iter()
        .map(|value| value.as_str().expect("string enum value").to_owned())
        .collect()
}

#[test]
fn clean_action_enum_matches_the_documented_actions() {
    let values = enum_values(&hygiene_def("hygiene_clean"), "action");
    assert_eq!(
        values,
        ["flag", "archive", "discard", "delete", "apply_suggested"]
    );
}

#[test]
fn clean_description_names_every_offered_action() {
    let def = hygiene_def("hygiene_clean");
    for value in enum_values(&def, "action") {
        assert!(
            def.description.contains(value.as_str()),
            "`{}` does not document `{value}`",
            def.description
        );
    }
}

#[test]
fn no_hygiene_schema_value_offers_keep_as_an_action() {
    for def in hygiene_defs() {
        for (name, schema) in &def.props {
            let Some(values) = schema["enum"].as_array() else {
                continue;
            };
            assert!(
                !values.iter().any(|value| value.as_str() == Some("keep")),
                "`{}` property `{name}` still offers the removed action",
                def.name
            );
        }
    }
}
