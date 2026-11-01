// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the portability tool definitions this
//! module publishes: their names, the single argument each writing tool
//! requires, and the confinement both destination descriptions state.
//!
//! ## Owns
//! - Every test of `portability_defs`
//!
//! ## Does not own
//! - Pack assembly or schema validation (see `schema_defs_tests`)
//! - Destination confinement itself (see `export_destination_tests`)
//! - Tool dispatch (see `portability_tools_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Changes to a portability tool's parameter contract should normally update
//! tests here. Changes to dispatch should not.
//!
//! Keep this module focused on the published definitions.

use super::portability_defs;
use crate::schema_defs::ToolDef;

/// One definition by name, so a property check cannot pass by position.
fn find(name: &str) -> ToolDef {
    portability_defs()
        .into_iter()
        .find(|def| def.name == name)
        .unwrap_or_else(|| panic!("`{name}` is not a portability tool"))
}

#[test]
fn every_portability_tool_is_listed_by_name() {
    let names: Vec<&str> = portability_defs().iter().map(|def| def.name).collect();

    assert_eq!(
        names,
        vec!["export", "review_export", "import", "import_provider"]
    );
    for def in portability_defs() {
        assert!(
            !def.description.is_empty(),
            "{} has no description",
            def.name
        );
    }
}

/// One destination and nothing else: a bundle that also needed a bank, a
/// format, or a mode to be named would have no default that a harness can rely
/// on, and the handler reads only the destination.
#[test]
fn export_requires_exactly_the_destination() {
    let def = find("export");

    assert_eq!(def.required, &["destination"][..]);
    assert!(
        def.props.iter().any(|(name, _)| *name == "destination"),
        "export requires a destination it never declares"
    );
}

#[test]
fn review_export_requires_exactly_the_destination() {
    let def = find("review_export");

    assert_eq!(def.required, &["destination"][..]);
    assert!(
        def.props.iter().any(|(name, _)| *name == "destination"),
        "review_export requires a destination it never declares"
    );
}

/// Both writing tools refuse a destination outside the deployment's export
/// root, so both published descriptions must say where the path may land. A
/// client that learns the rule only from a runtime refusal has wasted a write.
#[test]
fn both_destination_descriptions_confine_the_path_to_the_export_root() {
    for name in ["export", "review_export"] {
        let tool = find(name).to_tool();
        let description = tool["inputSchema"]["properties"]["destination"]["description"]
            .as_str()
            .unwrap_or_else(|| panic!("{name} publishes no destination description"));

        assert!(
            description.contains("must lie inside the export root"),
            "the {name} destination description never confines the path: {description}"
        );
    }
}
