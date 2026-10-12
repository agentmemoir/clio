// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the help-topic contract: every
//! registered name resolves through the top level's own `clio help` gate.
//!
//! ## Owns
//! - The real registry's help-topic resolution, multi-token names and the
//!   visible `show` record included, and the unknown-name refusal
//!
//! ## Does not own
//! - The topic parser itself (`cli_top_level`) or the check under test
//!   (`cli_command_registry_help_topics`)
//! - The usage lines the topics print (`cli_help_usage`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the help-topic contract.

use super::check_help_topics;
use crate::cli_command_spec::CommandSpec;

/// Every visible record name resolves as a `clio help` topic.
#[test]
fn every_registered_name_resolves_as_a_help_topic() {
    let specs: Vec<&CommandSpec> = crate::cli_command_registry::all().collect();
    assert_eq!(check_help_topics(&specs), Ok(()));
}

/// Multi-token names and the visible `show` record resolve. The alias route
/// itself is owned by `cli_command_registry_tests`; this check reads
/// `spec.name`, so an alias removal cannot fail here.
#[test]
fn multi_token_names_and_the_visible_show_record_resolve() {
    for name in ["task get", "sync key export", "show"] {
        let spec = crate::cli_command_registry_test_support::record(name);
        assert_eq!(check_help_topics(&[spec]), Ok(()), "`clio help {name}`");
    }
}

/// A name the top level does not know fails the check, named in the message.
#[test]
fn an_unknown_name_fails_the_topic_check_naming_it() {
    let spec = CommandSpec {
        name: "ghost topic",
        ..crate::cli_command_registry_test_support::record("get").clone()
    };
    let error = check_help_topics(&[&spec]).expect_err("an unknown topic is rejected");
    assert!(error.contains("ghost topic"), "{error}");
}
