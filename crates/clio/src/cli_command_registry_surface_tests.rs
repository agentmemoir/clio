// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the reserved-surface tranche's record content: each
//! record's authored bytes against the authored usage rows, the subcommand
//! records' summaries and the top-level records' prose against their authored
//! rows, and the tranche's inventory contract.
//!
//! ## Owns
//! - Byte equality of each record's `usage_line` with the authored surface row
//! - The subcommand records' summaries against their authored catalog rows
//! - The help prose records carry and the accepted sets `version` and
//!   `health` declare
//! - The tranche's `validate` contract against the catalog inventory
//!
//! ## Does not own
//! - The parser and helper cases and the dispatch's trailing-argument and
//!   refusal routing (`cli_command_registry_surface_parse_tests`)
//! - The registry lookups and `validate` itself (`cli_command_registry_tests`)
//! - The reserved surfaces' end-to-end process contract
//!   (`reserved_surface_args_harness`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the reserved-surface records' data.

use crate::cli_command_registry_test_support::record;
use crate::cli_command_spec::CommandSpec;

/// The tranche's records, as the validation contract consumes them.
fn tranche() -> Vec<&'static CommandSpec> {
    super::SURFACE.iter().collect()
}

/// The tranche's records pass `validate` against the catalog inventory.
#[test]
fn the_surface_tranche_validates_cleanly() {
    let inventory: Vec<&str> = crate::command_catalog::catalog_commands().collect();
    assert_eq!(
        crate::cli_command_registry::validate(&tranche(), &inventory),
        Ok(())
    );
}

/// The tranche lists its fifteen records once each: the seven single
/// commands, then the eight `mcp` and `ops` subcommands.
#[test]
fn the_tranche_lists_its_fifteen_records_once_each() {
    let names: Vec<&str> = super::SURFACE.iter().map(|spec| spec.name).collect();
    assert_eq!(
        names,
        [
            "version",
            "health",
            "help",
            "status",
            "setup",
            "install",
            "uninstall",
            "mcp schema-export",
            "mcp stdio",
            "mcp http",
            "ops diagnose",
            "ops verify",
            "ops doctor",
            "ops repair",
            "ops reindex",
        ]
    );
}

/// The eight subcommand records keep the summary bytes the catalog printed
/// before the records: same key, same row text, so the catalog renders
/// unchanged.
#[test]
fn the_subcommand_records_keep_their_authored_catalog_rows() {
    for name in [
        "mcp schema-export",
        "mcp stdio",
        "mcp http",
        "ops diagnose",
        "ops verify",
        "ops doctor",
        "ops repair",
        "ops reindex",
    ] {
        let authored = crate::command_catalog::ENTRIES_BEFORE
            .iter()
            .find(|(entry, _)| *entry == name)
            .map_or_else(
                || panic!("`{name}` has an authored catalog row"),
                |(_, summary)| *summary,
            );
        assert_eq!(record(name).summary, authored, "{name} catalog row");
    }
}

/// Every record answers the lookups, and each line is the bytes the authored
/// surface row held, so the catalog and `--help` render unchanged.
///
/// The first two are existence pins only: both lookups answer a registered
/// command from its record first, so comparing their text with the same
/// record's text would hold by construction. The independent byte evidence is
/// the third assertion, which reads the frozen authored row.
#[test]
fn every_record_answers_the_lookups_with_the_authored_row_bytes() {
    for spec in &tranche() {
        assert!(
            crate::cli_help_usage::usage_for(spec.name).is_some(),
            "{} resolves through the usage lookup",
            spec.name
        );
        assert!(
            crate::cli_help_usage::usage_line(spec.name).is_some(),
            "{} usage line resolves through the lookup",
            spec.name
        );
        assert_eq!(
            crate::cli_help_surfaces::surface_usage_line(spec.name),
            Some(spec.usage_line().as_str()),
            "{} keeps the authored surface row's bytes",
            spec.name
        );
    }
}

/// `version` and `health` carry their help prose on the record and accept no
/// flag of their own: every extra they name is a declared global.
#[test]
fn version_and_health_carry_their_help_prose_and_no_own_flags() {
    assert_eq!(
        record("version").notes,
        ["Prints the package name and version. Takes no flags."]
    );
    assert_eq!(
        record("health").notes,
        [
            "Prints the machine-readable health JSON: name and version only.\nIt opens no database and checks no service."
        ]
    );
    for name in ["version", "health"] {
        let spec = record(name);
        assert!(spec.documented.is_empty(), "{name} documents no flag");
        for extra in spec.accepted_extra {
            assert!(
                crate::cli_command_registry::global_flag(extra).is_some(),
                "{name} names --{extra}, which is not a declared global"
            );
        }
    }
}
