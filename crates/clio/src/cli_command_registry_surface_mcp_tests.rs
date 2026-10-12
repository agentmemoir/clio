// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the three `mcp` records: their completeness contract and
//! the `schema-export` flag-alias pair that leaves both spellings documented.
//!
//! ## Owns
//! - The `check_metadata` result over the three `mcp` records
//! - The `schema-export` alias pin (`path -> out`), both documented
//!   spellings, and the examples that show each one
//! - The transports' declaration of no flag alias
//!
//! ## Does not own
//! - The tranche-level inventory and byte pins (see
//!   `cli_command_registry_surface_tests`)
//! - The parser cases (see `cli_command_registry_surface_parse_tests`)
//! - The registry lookups and `validate` itself (`cli_command_registry_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the three `mcp` records' data.

use crate::cli_command_registry_test_support::record;
use crate::cli_command_spec::CommandSpec;

/// The three `mcp` records pass the completeness contract.
#[test]
fn the_mcp_records_carry_complete_metadata() {
    let specs: Vec<&CommandSpec> = ["mcp schema-export", "mcp stdio", "mcp http"]
        .iter()
        .map(|name| record(name))
        .collect();
    assert_eq!(crate::cli_command_registry::check_metadata(&specs), Ok(()));
}

/// `mcp schema-export` declares exactly the `path -> out` flag alias, keeps
/// both spellings documented, and shows each one in an example; the two
/// transports declare no flag alias.
#[test]
fn the_schema_export_alias_is_path_to_out_and_the_transports_declare_none() {
    let schema_export = record("mcp schema-export");
    assert_eq!(
        schema_export.flag_aliases,
        [("path", "out")],
        "the alias pair"
    );
    for spelling in ["out", "path"] {
        assert!(
            schema_export
                .documented
                .iter()
                .any(|flag| flag.long == spelling),
            "--{spelling} stays documented"
        );
        assert!(
            schema_export
                .examples
                .iter()
                .any(|example| example.command.contains(&format!("--{spelling} "))),
            "--{spelling} is shown by an example"
        );
    }
    for name in ["mcp stdio", "mcp http"] {
        assert!(
            record(name).flag_aliases.is_empty(),
            "{name} declares no flag alias"
        );
    }
}
