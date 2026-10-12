// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared test helpers used by the
//! `cli_command_registry_*` test suites.
//!
//! ## Owns
//! - `record`, the registered-record fetch that fails naming the missing command
//! - `positionals`, the `(label, required)` projection the tranche tests pin
//! - The example-count pin and the example-names-its-record pin
//!
//! ## Does not own
//! - The records themselves (see the `cli_command_registry_*` modules)
//! - The per-suite assertions over them
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the shared registry lookups and pins.

use crate::cli_command_spec::CommandSpec;

/// One registered record, or a failure that names the missing command.
pub(crate) fn record(name: &str) -> &'static CommandSpec {
    crate::cli_command_registry::find(name).unwrap_or_else(|| panic!("`{name}` is registered"))
}

/// The `(label, required)` pairs of a record's positionals, as the usage line
/// renders them: the tranche tests pin this shape, and `check_metadata` only
/// requires each positional description to be non-blank.
pub(crate) fn positionals(spec: &CommandSpec) -> Vec<(&'static str, bool)> {
    spec.positionals
        .iter()
        .map(|positional| (positional.label, positional.required))
        .collect()
}

/// The record carries two to four authored examples: the range every tranche
/// pins, so an empty or shrunken list fails here with the count named.
pub(crate) fn assert_example_count(name: &str, spec: &CommandSpec) {
    assert!(
        (2..=4).contains(&spec.examples.len()),
        "{name} carries {} examples",
        spec.examples.len()
    );
}

/// Every authored example names its own record: `clio <name>` alone or
/// `clio <name> ...`, so a copied example fails naming its record.
pub(crate) fn assert_examples_name_their_record(name: &str, spec: &CommandSpec) {
    let own = format!("clio {}", spec.name);
    for example in spec.examples {
        assert!(
            example.command == own || example.command.starts_with(&format!("{own} ")),
            "{name} example `{}` does not name its record",
            example.command
        );
    }
}
