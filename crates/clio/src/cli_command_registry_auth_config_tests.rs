// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the seven auth and config records
//! against the usage bytes and parser flag sets the commands print today, for
//! pinning the `auth token add` override and its two `--bank` display entries,
//! and for the dispatcher completeness scan over the `auth` and `config`
//! prefixes.
//!
//! ## Owns
//! - Byte equality of each record's `full_usage` and `usage_line` with the
//!   registered usage text
//! - The exact `parser_spec` value and boolean flag sequences per command
//! - Each record's summary, tool, section, positional, and required-flag values
//! - The override pin: `auth token add` is the tranche's only pinned line, and
//!   its parser still collects every `--bank` occurrence
//! - The scan pin: every `auth` and `config` leaf is registered exactly once in
//!   capture order, and the config dispatcher's verb list is the registry's
//!
//! ## Does not own
//! - The registry lookups and validation (`cli_command_registry_tests`)
//! - The frozen golden capture (`golden_repeatability_test`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the seven auth and config records' data.

use crate::cli_command_registry_test_support::{
    assert_example_count, assert_examples_name_their_record, record,
};
use crate::cli_command_spec::{CommandSpec, Positional, Section};

/// `auth token add` renders today's pinned line and note, and its parser keeps
/// the globals merged with its own flags.
#[test]
fn auth_token_add_matches_the_registered_usage_and_parser() {
    let spec = record("auth token add");
    assert_eq!(
        spec.full_usage(),
        "usage: clio auth token add --label LABEL --actor ACTOR --bank BANK [--bank BANK] \
         [--output json|text]\n       generates a credential, prints its secret exactly once, \
         and stores only the digest in the owner-only token store; the secret cannot be \
         recovered — save it when you see it; the store file is held under a lock for the \
         whole write, so a run that cannot take the lock refuses without printing a secret, \
         and --db and --backend are refused because they cannot change what is written",
        "auth token add full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio auth token add --label LABEL --actor ACTOR --bank BANK [--bank BANK] \
         [--output json|text]",
        "auth token add usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "label"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.required_flags(), ["label", "actor", "bank"]);
    assert_eq!(spec.tool, None);
    assert_eq!(spec.section, Section::Write);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// `auth token list` renders today's usage text and builds today's parser sets.
#[test]
fn auth_token_list_matches_the_registered_usage_and_parser() {
    let spec = record("auth token list");
    assert_eq!(
        spec.full_usage(),
        "usage: clio auth token list [--output json|text]\n       prints labels, actors, \
         banks, and creation times; never a secret and never a digest; --db, --backend, \
         --actor, and --bank are refused because they cannot change the listing",
        "auth token list full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio auth token list [--output json|text]",
        "auth token list usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.tool, None);
    assert_eq!(spec.section, Section::Read);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// `auth token revoke` renders today's usage text and builds today's parser
/// sets; `--label` is the one flag the line writes bare.
#[test]
fn auth_token_revoke_matches_the_registered_usage_and_parser() {
    let spec = record("auth token revoke");
    assert_eq!(
        spec.full_usage(),
        "usage: clio auth token revoke --label LABEL [--output json|text]\n       removes the \
         record; servers load the store at start, so the change takes effect at the next \
         server start; the write holds the store lock, so it never drops a record a \
         concurrent add wrote, and --db, --backend, --actor, and --bank are refused",
        "auth token revoke full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio auth token revoke --label LABEL [--output json|text]",
        "auth token revoke usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "label"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.required_flags(), ["label"], "the bare flag");
    assert_eq!(spec.tool, None);
    assert_eq!(spec.section, Section::Write);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// The override keeps both `--bank` display positions while the parser keeps
/// collecting every occurrence of the one accepted name: the record documents
/// the required and the optional entry in order, and two parsed banks survive.
#[test]
fn the_add_record_keeps_both_bank_display_positions_and_the_parser_collects_them() {
    let spec = record("auth token add");
    let pinned = spec.usage_override.expect("the add line is pinned");
    assert_eq!(
        pinned,
        "clio auth token add --label LABEL --actor ACTOR --bank BANK [--bank BANK] \
         [--output json|text]"
    );
    let banks: Vec<&str> = spec
        .documented
        .iter()
        .filter(|flag| flag.long == "bank")
        .map(|flag| flag.long)
        .collect();
    assert_eq!(banks, ["bank", "bank"], "both display entries stay ordered");
    assert!(spec.documented[2].required, "the first entry is required");
    assert!(spec.documented[2].repeatable, "the parser repeats the flag");
    assert!(!spec.documented[3].required, "the second entry is optional");
    let tokens: Vec<String> = ["--bank", "a", "--bank", "b", "--label", "l"]
        .iter()
        .map(|token| (*token).to_owned())
        .collect();
    let parsed = crate::cli_args::parse(&spec.parser_spec(), tokens).expect("two banks parse");
    assert_eq!(
        parsed.values_of("bank"),
        ["a".to_owned(), "b".to_owned()],
        "every occurrence is kept in order"
    );
    assert_eq!(parsed.value("label"), Some("l"));
}

/// `config get` renders today's usage text and builds today's parser sets.
#[test]
fn config_get_matches_the_registered_usage_and_parser() {
    let spec = record("config get");
    assert_eq!(
        spec.full_usage(),
        "usage: clio config get [PATH] [--output json|text]",
        "config get full usage"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.tool, Some("config_get"));
    assert_eq!(spec.section, Section::Read);
    assert_eq!(
        spec.positionals,
        [Positional {
            label: "PATH",
            required: false,
            description: "Dotted configuration path `config get` reads; with no path the whole effective configuration prints."
        }]
        .as_slice()
    );
}

/// `config set` renders today's usage text and builds today's parser sets.
#[test]
fn config_set_matches_the_registered_usage_and_parser() {
    let spec = record("config set");
    assert_eq!(
        spec.full_usage(),
        "usage: clio config set PATH VALUE [--scope session|profile|deployment] \
         [--output json|text]",
        "config set full usage"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "scope"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.tool, Some("config_set"));
    assert_eq!(spec.section, Section::Write);
    assert_eq!(
        spec.positionals,
        [
            Positional {
                label: "PATH",
                required: true,
                description: "Dotted configuration path `config set` writes; the `config_set` tool validates it before applying."
            },
            Positional {
                label: "VALUE",
                required: true,
                description: "Value `config set` stores at the path; decoded as JSON when it parses, otherwise stored as text."
            },
        ]
        .as_slice()
    );
}

/// `config profiles` and `config use` render today's usage text and build
/// today's parser sets.
#[test]
fn config_profiles_and_use_match_the_registered_usage_and_parser() {
    let profiles = record("config profiles");
    assert_eq!(
        profiles.full_usage(),
        "usage: clio config profiles [--output json|text]",
        "config profiles full usage"
    );
    assert_eq!(
        profiles.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(profiles.tool, Some("config_profiles"));
    assert_eq!(profiles.section, Section::Read);
    assert!(
        profiles.positionals.is_empty(),
        "{:?}",
        profiles.positionals
    );

    let use_record = record("config use");
    assert_eq!(
        use_record.full_usage(),
        "usage: clio config use NAME [--output json|text]",
        "config use full usage"
    );
    assert_eq!(
        use_record.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(use_record.tool, Some("config_profile_apply"));
    assert_eq!(use_record.section, Section::Write);
    assert_eq!(
        use_record.positionals,
        [Positional {
            label: "NAME",
            required: true,
            description: "Profile name `config use` applies for later commands."
        }]
        .as_slice()
    );
}

/// Every `auth` and `config` leaf is registered exactly once, in the order the
/// commands were authored: the three token verbs, then the four config verbs.
/// A missing leaf, a duplicate registration, or a reorder fails here.
#[test]
fn the_auth_and_config_leaves_are_registered_once_in_capture_order() {
    let expected = [
        "auth token add",
        "auth token list",
        "auth token revoke",
        "config get",
        "config set",
        "config profiles",
        "config use",
    ];
    let listed: Vec<&str> = crate::cli_command_registry::names()
        .filter(|name| name.starts_with("auth ") || name.starts_with("config "))
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

/// The config dispatcher's verb list is the registry's, not a second list: a
/// verb the registry does not hold, or holds in another order, fails here.
#[test]
fn the_config_group_dispatches_the_registrys_verb_list() {
    use crate::runtime_cli::RuntimeGroup;
    let derived: Vec<&str> = crate::cli_command_registry::names()
        .filter(|name| name.starts_with("config "))
        .collect();
    assert_eq!(
        crate::cli_command_registry_auth_config::CONFIG_VERBS,
        derived.as_slice(),
        "the tranche's verb list must come from the records"
    );
    assert_eq!(
        crate::config_cli::ConfigGroup.verbs(),
        derived.as_slice(),
        "the dispatcher must dispatch the registry's verbs"
    );
}

/// The seven records' summaries are the catalog rows they replaced, and none
/// carries an alias, flag alias, or alternate; each carries two to four
/// examples naming its own command, the three auth records each hold one note,
/// and the four config records none.
#[test]
fn the_seven_records_carry_the_catalog_rows_and_no_extra_surfaces() {
    for (name, summary) in [
        (
            "auth token add",
            "Generate a network credential, print its secret exactly once, and store only the \
             digest in the owner-only token store; the secret cannot be recovered",
        ),
        (
            "auth token list",
            "Show labels, actors, banks, and created_at; never a secret or a digest",
        ),
        (
            "auth token revoke",
            "Remove the record; takes effect at the next server start",
        ),
        ("config get", "read one configuration path"),
        ("config set", "write one configuration path at a scope"),
        (
            "config profiles",
            "list the available configuration profiles",
        ),
        ("config use", "apply one profile by name"),
    ] {
        let spec = record(name);
        assert_eq!(spec.summary, summary, "{name} summary");
        assert!(spec.aliases.is_empty(), "{name} aliases");
        assert!(spec.flag_aliases.is_empty(), "{name} flag aliases");
        assert!(spec.usage_alternates.is_empty(), "{name} alternates");
        assert_example_count(name, spec);
        assert_examples_name_their_record(name, spec);
    }
    for name in ["auth token add", "auth token list", "auth token revoke"] {
        assert_eq!(record(name).notes.len(), 1, "{name} notes");
    }
    for name in ["config get", "config set", "config profiles", "config use"] {
        assert_eq!(record(name).notes.len(), 0, "{name} notes");
    }
}

/// A flag absent from a record's parser spec is still refused the way it is
/// today: `auth token list` has no `--patch` and `config get` has no `--scope`.
#[test]
fn a_flag_absent_from_a_record_is_still_refused() {
    let tokens =
        |flags: &[&str]| -> Vec<String> { flags.iter().map(|token| (*token).to_owned()).collect() };
    let error = crate::cli_args::parse(
        &record("auth token list").parser_spec(),
        tokens(&["--patch", "{}"]),
    )
    .expect_err("--patch is not a list flag");
    assert!(
        error.message().contains("unknown flag"),
        "{}",
        error.message()
    );
    let error = crate::cli_args::parse(
        &record("config get").parser_spec(),
        tokens(&["--scope", "session"]),
    )
    .expect_err("--scope is not a get flag");
    assert!(
        error.message().contains("unknown flag"),
        "{}",
        error.message()
    );
}

/// The real registry satisfies `validate` against the catalog inventory.
#[test]
fn validate_accepts_the_auth_and_config_records_against_the_catalog_inventory() {
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
fn the_auth_and_config_tranche_has_complete_flag_metadata() {
    let specs: Vec<&CommandSpec> = super::AUTH_CONFIG_LEAVES.iter().collect();
    assert_eq!(crate::cli_command_registry::check_metadata(&specs), Ok(()));
}

/// The `config set` scope metadata default is the session constant `config_cli`
/// falls back to when `--scope` is absent.
#[test]
fn the_config_scope_default_is_the_session_constant() {
    let scope = record("config set")
        .documented
        .iter()
        .find(|flag| flag.long == "scope")
        .expect("config set documents --scope");
    assert_eq!(
        scope.default,
        Some(clio_config::ConfigScope::Session.as_str())
    );
}
