// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the rendered help lookup: that it
//! answers with the record's own text plus the record's options block, that
//! flag descriptions and real defaults render, and that the shared globals
//! print through their shared definitions.
//!
//! ## Owns
//! - The rendered-lookup pin: `help_for` answers with `full_usage` plus the
//!   options block, exactly when the record has one to show
//! - The mechanism pins over every registered command: each documented flag's
//!   description renders, the `[default: ` suffix renders exactly when a flag
//!   sets one, and the `Global flags:` group renders exactly when extras are
//!   accepted
//! - The note-prose-before-options ordering pin
//!
//! ## Does not own
//! - The lookup-resolution and usage-line pins (`cli_help_usage_tests`)
//! - The options block's own rendering (`cli_command_spec_options_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the rendered help lookup.

use crate::cli_help_usage;

/// The rendered lookup answers with the record's own text, then the record's
/// options block when the record has one to show, and with nothing appended
/// when it does not.
#[test]
fn the_rendered_lookup_appends_the_record_options_block() {
    for spec in crate::cli_command_registry::all() {
        let rendered = cli_help_usage::help_for(spec.name).unwrap_or_else(|| {
            panic!("{} is registered but the lookup does not answer", spec.name)
        });
        let options = spec.options_help();
        let expected = if options.is_empty() {
            spec.full_usage()
        } else {
            format!("{}\n\n{options}", spec.full_usage())
        };
        assert_eq!(rendered, expected, "{}", spec.name);
    }
}

/// Every registered command's rendered help shows each documented flag's
/// description, and carries a `[default: ` suffix exactly when a documented
/// flag sets a default: the mechanism renders the record, whatever values the
/// records carry.
#[test]
fn every_registered_help_renders_its_flag_descriptions_and_defaults() {
    for spec in crate::cli_command_registry::all() {
        let usage = cli_help_usage::help_for(spec.name).unwrap_or_else(|| {
            panic!("{} is registered but the lookup does not answer", spec.name)
        });
        for flag in spec.documented {
            assert!(
                usage.contains(flag.description),
                "{}: --{}'s description does not render: {usage}",
                spec.name,
                flag.long
            );
        }
        assert_eq!(
            usage.contains(" [default: "),
            spec.documented.iter().any(|flag| flag.default.is_some()),
            "{}: the default suffix must render exactly when a flag sets one: {usage}",
            spec.name
        );
    }
}

/// The extra group prints through the shared `GLOBAL_*` definitions: a record
/// with accepted extras shows a `Global flags:` block naming each one with the
/// shared definition's own description, and a record with none shows none.
#[test]
fn the_global_flags_group_prints_the_shared_definitions() {
    for spec in crate::cli_command_registry::all() {
        let usage = cli_help_usage::help_for(spec.name).unwrap_or_else(|| {
            panic!("{} is registered but the lookup does not answer", spec.name)
        });
        assert_eq!(
            usage.contains("Global flags:"),
            !spec.accepted_extra.is_empty(),
            "{}: the extra group must print exactly when extras are accepted: {usage}",
            spec.name
        );
        for name in spec.accepted_extra {
            let flag = crate::cli_command_registry::global_flag(name)
                .expect("accepted_extra names a declared global flag");
            assert!(
                usage.contains(&format!("--{name}")),
                "{}: {usage}",
                spec.name
            );
            assert!(
                usage.contains(flag.description),
                "{}: {name} must render its shared description: {usage}",
                spec.name
            );
        }
    }
}

/// The options block prints after the record's note prose, so the explanation
/// a reader meets first stays beside the usage line it explains. `recall`
/// carries the note this needs without pinning any tranche's wording.
#[test]
fn the_options_block_prints_after_the_note_prose() {
    let usage = cli_help_usage::help_for("recall").expect("recall has a usage line");
    let note_at = usage
        .lines()
        .position(|line| line.starts_with("       ") && !line.starts_with("       or:"))
        .expect("recall carries a note");
    let options_at = usage.find("\nOptions:").expect("the options block prints");
    assert!(note_at < options_at, "{usage}");
}
