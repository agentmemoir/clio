// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the seven reserved single commands: their completeness
//! contract and the `help` record's topic positional and variants.
//!
//! ## Owns
//! - The `check_metadata` result over the seven single records
//! - The `help` record's described, optional `TOPIC` positional and its
//!   catalog, topic, and JSON-catalog examples
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
//! Keep this module focused on the seven single-command records' data.

use crate::cli_command_registry_test_support::record;
use crate::cli_command_spec::CommandSpec;

/// The names of the seven single commands, in catalog order.
const NAMES: &[&str] = &[
    "version",
    "health",
    "help",
    "status",
    "setup",
    "install",
    "uninstall",
];

/// The seven single commands pass the completeness contract: every flag and
/// positional is described, and every record carries two to four examples its
/// own parser accepts.
#[test]
fn the_singles_records_carry_complete_metadata() {
    let specs: Vec<&CommandSpec> = NAMES.iter().map(|name| record(name)).collect();
    assert_eq!(crate::cli_command_registry::check_metadata(&specs), Ok(()));
}

/// The `help` record's `TOPIC` positional is optional and described, and the
/// record keeps one example for each contract shape: the plain catalog, a
/// topic, and the JSON catalog.
#[test]
fn the_help_record_describes_its_topic_and_keeps_its_variants() {
    let spec = record("help");
    let topic = spec.positionals.first().expect("help carries TOPIC");
    assert_eq!(topic.label, "TOPIC");
    assert!(!topic.required, "TOPIC is optional");
    assert!(
        !topic.description.trim().is_empty(),
        "TOPIC is described for the catalog"
    );
    let commands: Vec<&str> = spec
        .examples
        .iter()
        .map(|example| example.command)
        .collect();
    assert_eq!(
        commands,
        [
            "clio help",
            "clio help backup",
            "clio help --json",
            "clio help --markdown --out ./manual",
        ],
        "the help record's examples"
    );
}
