// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the five ranking and retention
//! records against the usage bytes and parser flag sets the commands print
//! today, for pinning the retention records' own flags against the authored
//! surface rows, and for the dispatcher completeness scan over the `ranking`
//! and `retention` prefixes.
//!
//! ## Owns
//! - Byte equality of each record's `full_usage` and `usage_line` with the
//!   registered usage text, the retention lines read from the authored surface
//!   rows they replaced
//! - The exact `parser_spec` value and boolean flag sequences per command
//! - Each record's summary, tool, section, and required-flag values
//! - The scan pin: every `ranking` and `retention` leaf is registered exactly
//!   once in capture order, the ranking dispatcher's verb list is the
//!   registry's, and the retention parser's known flags are the record's
//!
//! ## Does not own
//! - The registry lookups and validation (`cli_command_registry_tests`)
//! - The frozen golden capture (`golden_repeatability_test`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the five ranking and retention records' data.

use crate::cli_command_registry_test_support::record;
use crate::cli_command_spec::{CommandSpec, Section};

/// `ranking get` renders today's usage text and builds today's parser sets.
#[test]
fn ranking_get_matches_the_registered_usage_and_parser() {
    let spec = record("ranking get");
    assert_eq!(
        spec.full_usage(),
        "usage: clio ranking get [--output json|text]",
        "ranking get full usage"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.tool, Some("ranking_env_get"));
    assert_eq!(spec.section, Section::Read);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// `ranking set` renders today's usage text and builds today's parser sets;
/// `--patch` is the one flag the line writes bare.
#[test]
fn ranking_set_matches_the_registered_usage_and_parser() {
    let spec = record("ranking set");
    assert_eq!(
        spec.full_usage(),
        "usage: clio ranking set --patch JSON [--patch-file FILE] [--dry-run] \
         [--output json|text]",
        "ranking set full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio ranking set --patch JSON [--patch-file FILE] [--dry-run] [--output json|text]",
        "ranking set usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db",
            "backend",
            "bank",
            "actor",
            "output",
            "patch",
            "patch-file"
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "dry-run"]
    );
    assert_eq!(spec.required_flags(), ["patch"], "the bare flag");
    assert_eq!(spec.tool, Some("ranking_env_set"));
    assert_eq!(spec.section, Section::Write);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// The three retention records' lines are the authored surface rows they
/// replaced, byte for byte, and each renders as the same `usage:` text.
#[test]
fn the_retention_lines_are_the_authored_surface_rows() {
    for name in ["retention get", "retention set", "retention schema"] {
        let authored = crate::cli_help_surfaces::surface_usage_line(name)
            .unwrap_or_else(|| panic!("`{name}` keeps its authored surface row"));
        let spec = record(name);
        assert_eq!(spec.usage_line(), authored, "{name} usage line");
        assert_eq!(spec.full_usage(), format!("usage: {authored}"), "{name}");
        assert!(spec.notes.is_empty(), "{name} notes");
    }
}

/// The retention records' own flags are the flags their lines print, in print
/// order, and `retention set`'s `--profile` is the one bare flag.
#[test]
fn the_retention_records_carry_their_own_flags_in_print_order() {
    let longs = |name: &str| -> Vec<&'static str> {
        record(name)
            .documented
            .iter()
            .map(|flag| flag.long)
            .collect()
    };
    assert_eq!(longs("retention get"), ["bank", "profiles"]);
    assert_eq!(
        longs("retention set"),
        ["bank", "scope", "profile", "profile-file", "profiles"]
    );
    assert_eq!(longs("retention schema"), &[] as &[&str]);
    assert_eq!(
        record("retention set").required_flags(),
        ["profile"],
        "the bare flag"
    );
    for name in ["retention get", "retention set", "retention schema"] {
        let spec = record(name);
        // The shared parse rule admits every declared global, so the record
        // names the ones its line does not print, the way every parse-gated
        // record does: six past its documented `--bank`, and all seven for
        // the schema leaf, whose line names no flag.
        let wanted = if name == "retention schema" { 7 } else { 6 };
        assert_eq!(spec.accepted_extra.len(), wanted, "{name} accepted extras");
        for extra in spec.accepted_extra {
            assert!(
                crate::cli_command_registry::global_flag(extra).is_some(),
                "{name} names --{extra}, which is not a declared global"
            );
            assert!(
                !spec.documented.iter().any(|flag| flag.long == *extra),
                "{name} names --{extra} and also documents it"
            );
        }
        assert!(spec.positionals.is_empty(), "{name} positionals");
        assert_eq!(spec.tool, None, "{name} tool");
    }
    assert_eq!(record("retention get").section, Section::Read);
    assert_eq!(record("retention set").section, Section::Write);
    assert_eq!(record("retention schema").section, Section::Read);
}

/// The retention surface's parser accepts the record's own flags and refuses a
/// flag neither the record nor the declared globals name, the way it does
/// today. The known list is the production derivation, not a rebuild, so a
/// wrong derivation fails here.
#[test]
fn the_retention_parser_takes_the_records_own_flags() {
    for (sub, expected) in [
        ("get", &["bank", "profiles"][..]),
        (
            "set",
            &["bank", "scope", "profile", "profile-file", "profiles"][..],
        ),
        ("schema", &[] as &[&str]),
    ] {
        assert_eq!(
            crate::retention_cli::retention_known_flags(sub),
            expected,
            "retention {sub}'s derived known flags"
        );
    }
    let known = crate::retention_cli::retention_known_flags("get");
    let parsed = crate::cli_top_level::parse_flags(["--bank".to_owned(), "b".to_owned()], &known)
        .expect("--bank is one of the record's flags");
    assert_eq!(parsed.get("bank").map(String::as_str), Some("b"));
    let error = crate::cli_top_level::parse_flags(["--bogus".to_owned()], &known)
        .expect_err("--bogus is neither a record flag nor a global");
    assert!(error.message().contains("--bogus"), "{}", error.message());
}

/// The retention subcommand list is the production derivation of the records'
/// one-token suffixes: each current record contributes its own token, once, in
/// registry order, and none is skipped.
#[test]
fn the_retention_subcommands_are_the_records_one_token_suffixes() {
    assert_eq!(
        crate::retention_cli::retention_subcommands(),
        ["get", "set", "schema"],
        "the derived retention subcommands"
    );
}

/// Every `ranking` and `retention` leaf is registered exactly once, in the
/// order the commands were authored: `ranking get`, `ranking set`, then the
/// three retention subcommands. A missing leaf, a duplicate registration, or a
/// reorder fails here.
#[test]
fn the_ranking_and_retention_leaves_are_registered_once_in_capture_order() {
    let expected = [
        "ranking get",
        "ranking set",
        "retention get",
        "retention set",
        "retention schema",
    ];
    let listed: Vec<&str> = crate::cli_command_registry::names()
        .filter(|name| name.starts_with("ranking ") || name.starts_with("retention "))
        .collect();
    assert_eq!(listed, expected, "the leaves' registry order changed");
    for name in expected {
        assert_eq!(
            crate::cli_command_registry::all()
                .filter(|spec| spec.name == name)
                .count(),
            1,
            "`{name}` must be registered exactly once"
        );
    }
}

/// The ranking dispatcher's verb list is the registry's, not a second list: a
/// verb the registry does not hold, or holds in another order, fails here.
#[test]
fn the_ranking_group_dispatches_the_registrys_verb_list() {
    use crate::runtime_cli::RuntimeGroup;
    let derived: Vec<&str> = crate::cli_command_registry::names()
        .filter(|name| name.starts_with("ranking "))
        .collect();
    assert_eq!(
        crate::cli_command_registry_ranking_retention::RANKING_VERBS,
        derived.as_slice(),
        "the tranche's verb list must come from the records"
    );
    assert_eq!(
        crate::ranking_cli::RankingGroup.verbs(),
        derived.as_slice(),
        "the dispatcher must dispatch the registry's verbs"
    );
}

/// The five records' summaries are the catalog rows they replaced, and none
/// carries an alias, flag alias, alternate, or example.
#[test]
fn the_five_records_carry_the_catalog_rows_and_no_extra_surfaces() {
    for (name, summary) in [
        ("ranking get", "read the ranking environment"),
        ("ranking set", "patch the ranking environment"),
        (
            "retention get",
            "Show the effective per-bank retention profile",
        ),
        (
            "retention set",
            "Store a validated retention profile (persists to --profiles/env)",
        ),
        (
            "retention schema",
            "Print the retention profile JSON Schema",
        ),
    ] {
        let spec = record(name);
        assert_eq!(spec.summary, summary, "{name} summary");
        assert!(spec.aliases.is_empty(), "{name} aliases");
        assert!(spec.flag_aliases.is_empty(), "{name} flag aliases");
        assert!(spec.usage_alternates.is_empty(), "{name} alternates");
        assert!(spec.examples.is_empty(), "{name} examples");
    }
}

/// A flag absent from a record's parser spec is still refused the way it is
/// today: `ranking get` has no `--patch`.
#[test]
fn a_flag_absent_from_a_ranking_record_is_still_refused() {
    let tokens: Vec<String> = ["--patch", "{}"].iter().map(|t| (*t).to_owned()).collect();
    let error = crate::cli_args::parse(&record("ranking get").parser_spec(), tokens)
        .expect_err("--patch is not a get flag");
    assert!(
        error.message().contains("unknown flag"),
        "{}",
        error.message()
    );
}

/// The real registry satisfies `validate` against the catalog inventory.
#[test]
fn validate_accepts_the_ranking_and_retention_records_against_the_catalog_inventory() {
    let specs: Vec<&crate::cli_command_spec::CommandSpec> =
        crate::cli_command_registry::all().collect();
    let inventory: Vec<&str> = crate::command_catalog::catalog_commands().collect();
    assert_eq!(
        crate::cli_command_registry::validate(&specs, &inventory),
        Ok(())
    );
}

/// Every documented flag in the tranche carries complete metadata: a
/// description, and for a value flag a placeholder and a non-empty value kind.
#[test]
fn the_ranking_and_retention_tranche_has_complete_flag_metadata() {
    let specs: Vec<&CommandSpec> = super::RANKING_RETENTION_LEAVES.iter().collect();
    assert_eq!(crate::cli_command_registry::check_metadata(&specs), Ok(()));
}

/// The retention metadata defaults are the literals `retention_cli` falls back
/// to: `bank_of` uses `default`, and `run` uses `bank`.
#[test]
fn the_retention_defaults_match_the_retention_cli_fallbacks() {
    let default_of = |name: &str, long: &str| {
        record(name)
            .documented
            .iter()
            .find(|flag| flag.long == long)
            .unwrap_or_else(|| panic!("`{name}` documents --{long}"))
            .default
    };
    assert_eq!(default_of("retention get", "bank"), Some("default"));
    assert_eq!(default_of("retention set", "bank"), Some("default"));
    assert_eq!(default_of("retention set", "scope"), Some("bank"));
}
