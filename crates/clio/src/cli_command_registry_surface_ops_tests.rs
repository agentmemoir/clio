// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the five `ops` records: their completeness contract and
//! the confirmation and target gates their examples carry.
//!
//! ## Owns
//! - The `check_metadata` result over the five `ops` records
//! - The example pins: every `ops repair` example names `--confirm`, and every
//!   `ops reindex` example names `--target`
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
//! Keep this module focused on the five `ops` records' data.

use crate::cli_command_registry_test_support::record;
use crate::cli_command_spec::CommandSpec;

/// The five `ops` records pass the completeness contract.
#[test]
fn the_ops_records_carry_complete_metadata() {
    let specs: Vec<&CommandSpec> = [
        "ops diagnose",
        "ops verify",
        "ops doctor",
        "ops repair",
        "ops reindex",
    ]
    .iter()
    .map(|name| record(name))
    .collect();
    assert_eq!(crate::cli_command_registry::check_metadata(&specs), Ok(()));
}

/// Every `ops repair` example carries its `--confirm` gate, because live
/// repair refuses without it, and every `ops reindex` example names the target
/// leg it reports or rebuilds.
#[test]
fn the_ops_examples_carry_their_confirmation_and_target_gates() {
    for example in record("ops repair").examples {
        assert!(
            example.command.contains(" --confirm"),
            "`{}` must carry the confirm gate",
            example.command
        );
    }
    for example in record("ops reindex").examples {
        assert!(
            example.command.contains(" --target "),
            "`{}` must name its target",
            example.command
        );
    }
}
