// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the command registry: its order, its
//! lookups, its global flags, and the `validate` static-data contract.
//!
//! ## Owns
//! - `all`, `names`, `find`, `resolve`, and `aliases` over the real registry
//! - `validate` accepted against the catalog inventory, and every rejected
//!   bad state over pure records
//! - The global flag names, order, and short spelling
//!
//! ## Does not own
//! - The five records' bytes and parser sets
//!   (`cli_command_registry_core_reads_tests`)
//! - The renderer's shapes over pure records (`cli_command_spec_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the registry's lookups and validation.

use std::collections::BTreeSet;

use super::{
    aliases, all, find, global_bool_names, global_flag, global_value_names, names, resolve,
    resolve_in, validate,
};
use crate::cli_command_spec::{CommandSpec, Flag, Section};

/// A pure record with only the name, aliases, and tool these tests vary.
const fn record(
    name: &'static str,
    aliases: &'static [&'static str],
    tool: Option<&'static str>,
) -> CommandSpec {
    CommandSpec {
        name,
        aliases,
        summary: "",
        notes: &[],
        positionals: &[],
        documented: &[],
        accepted_extra: &[],
        flag_aliases: &[],
        usage_override: None,
        usage_alternates: &[],
        examples: &[],
        tool,
        section: Section::Read,
    }
}

/// The tranche lists the five core reads once each, in catalog order.
#[test]
fn all_yields_the_five_core_reads_once_in_catalog_order() {
    let listed: Vec<&str> = all().map(|spec| spec.name).collect();
    assert_eq!(listed, ["recall", "get", "show", "inspect", "stats"]);
    let unique: BTreeSet<&str> = listed.iter().copied().collect();
    assert_eq!(unique.len(), listed.len(), "no command is registered twice");
}

/// `names` and `find` answer for the visible records only.
#[test]
fn names_and_find_expose_the_visible_records() {
    assert_eq!(
        names().collect::<Vec<_>>(),
        ["recall", "get", "show", "inspect", "stats"]
    );
    assert_eq!(find("get").map(|spec| spec.name), Some("get"));
    assert_eq!(find("fetch"), None);
}

/// A name reaches its record through `resolve`; an unknown spelling does not.
#[test]
fn resolve_answers_visible_names_and_reports_unknown_spellings() {
    assert_eq!(resolve("show").map(|spec| spec.name), Some("show"));
    assert_eq!(resolve("get").map(|spec| spec.name), Some("get"));
    assert_eq!(resolve("fetch"), None);
}

/// `resolve_in` reaches a record through an alias, prefers a visible name over
/// another record's alias of the same string, and reports a miss as `None`.
#[test]
fn resolve_in_answers_aliases_and_prefers_a_visible_name() {
    let get = record("get", &["fetch", "show"], None);
    let show = record("show", &[], None);
    let specs = [&get, &show];

    assert_eq!(
        resolve_in(specs.iter().copied(), "fetch").map(|spec| spec.name),
        Some("get")
    );
    assert_eq!(
        resolve_in(specs.iter().copied(), "show").map(|spec| spec.name),
        Some("show")
    );
    assert_eq!(resolve_in(specs.iter().copied(), "missing"), None);
}

/// No registered record declares an alias yet, so `aliases` is empty.
#[test]
fn aliases_reports_the_declared_aliases() {
    assert_eq!(aliases().count(), 0);
}

/// The real registry satisfies `validate` against the catalog inventory.
#[test]
fn validate_accepts_the_registry_against_the_catalog_inventory() {
    let specs: Vec<&CommandSpec> = all().collect();
    let inventory: Vec<&str> = crate::command_catalog::catalog_commands().collect();
    assert_eq!(validate(&specs, &inventory), Ok(()));
}

/// The global flags keep the names and merge order every parser spec relies on.
#[test]
fn the_global_flags_keep_their_names_and_order() {
    assert_eq!(
        global_value_names().collect::<Vec<_>>(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(global_bool_names().collect::<Vec<_>>(), ["no-input", "yes"]);
    let output = global_flag("output").expect("output is declared");
    assert_eq!(output.short, Some('o'));
    assert!(global_flag("made-up").is_none());
}

/// Two records may not share a visible name.
#[test]
fn validate_rejects_a_duplicate_visible_name() {
    let first = record("recall", &[], None);
    let second = record("recall", &[], None);
    let error = validate(&[&first, &second], &["recall"]).expect_err("duplicates are rejected");
    assert!(error.contains("recall"), "{error}");
}

/// A visible name the catalog never lists is a row no surface can show.
#[test]
fn validate_rejects_a_visible_name_absent_from_the_inventory() {
    let ghost = record("ghost", &[], None);
    let error = validate(&[&ghost], &["recall"]).expect_err("an unknown name is rejected");
    assert!(error.contains("ghost"), "{error}");
}

/// An empty alias is a spelling nothing can type.
#[test]
fn validate_rejects_an_empty_alias() {
    let spec = record("get", &[""], None);
    let error = validate(&[&spec], &["get"]).expect_err("an empty alias is rejected");
    assert!(error.contains("get"), "{error}");
}

/// A record may not declare its own name as an alias.
#[test]
fn validate_rejects_an_alias_equal_to_its_own_name() {
    let spec = record("get", &["get"], None);
    let error = validate(&[&spec], &["get"]).expect_err("a self alias is rejected");
    assert!(error.contains("get"), "{error}");
}

/// Two records may not claim the same alias.
#[test]
fn validate_rejects_an_alias_claimed_by_two_records() {
    let first = record("get", &["fetch"], None);
    let second = record("show", &["fetch"], None);
    let error =
        validate(&[&first, &second], &["get", "show"]).expect_err("a shared alias is rejected");
    assert!(error.contains("fetch"), "{error}");
    assert!(error.contains("get"), "{error}");
    assert!(error.contains("show"), "{error}");
}

/// An alias that names a visible record of another tool shadows that record.
#[test]
fn validate_rejects_an_alias_that_shadows_a_different_tool_record() {
    let target = record("fetch", &[], Some("tool-fetch"));
    let aliasing = record("get", &["fetch"], Some("tool-get"));
    let error =
        validate(&[&target, &aliasing], &["fetch", "get"]).expect_err("shadowing is rejected");
    assert!(error.contains("shadow"), "{error}");
    assert!(error.contains("get"), "{error}");
}

/// Two records may not alias each other.
#[test]
fn validate_rejects_two_records_that_alias_each_other() {
    let first = record("get", &["show"], Some("get"));
    let second = record("show", &["get"], Some("get"));
    let error = validate(&[&first, &second], &["get", "show"]).expect_err("a cycle is rejected");
    assert!(error.contains("cycle"), "{error}");
    assert!(error.contains("get"), "{error}");
    assert!(error.contains("show"), "{error}");
}

/// An `accepted_extra` name that is not a declared global flag is a
/// static-data error that `validate` refuses before the parser can panic on it.
#[test]
fn validate_rejects_an_accepted_extra_that_is_not_a_declared_global() {
    let spec = CommandSpec {
        accepted_extra: &["made-up"],
        ..record("get", &[], None)
    };
    let error = validate(&[&spec], &["get"]).expect_err("an undeclared extra is rejected");
    assert!(error.contains("made-up"), "{error}");
}

/// A record may not both document a flag and list it as an accepted extra.
#[test]
fn validate_rejects_an_accepted_extra_that_is_also_documented() {
    const DOCUMENTED: &[Flag] = &[Flag::boolean("yes")];
    let spec = CommandSpec {
        documented: DOCUMENTED,
        accepted_extra: &["yes"],
        ..record("get", &[], None)
    };
    let error = validate(&[&spec], &["get"]).expect_err("a documented extra is rejected");
    assert!(error.contains("yes"), "{error}");
}

/// The intended visible-alias shape: a record aliases another visible record's
/// name, and both bind the same tool.
#[test]
fn validate_accepts_a_visible_alias_between_same_tool_records() {
    let get = record("get", &["show"], Some("get"));
    let show = record("show", &[], Some("get"));
    assert_eq!(validate(&[&get, &show], &["get", "show"]), Ok(()));
}

/// An alias that names no visible record is a plain extra spelling and passes.
#[test]
fn validate_accepts_a_plain_alias_that_names_no_record() {
    let get = record("get", &["fetch"], Some("get"));
    assert_eq!(validate(&[&get], &["get"]), Ok(()));
}

/// A record may document the same flag twice; `auth token add` needs this.
#[test]
fn validate_accepts_a_flag_documented_twice_by_one_record() {
    const DOCUMENTED: &[Flag] = &[Flag::value("bank", "BANK"), Flag::value("bank", "BANK")];
    let spec = CommandSpec {
        documented: DOCUMENTED,
        ..record("auth token add", &[], None)
    };
    assert_eq!(validate(&[&spec], &["auth token add"]), Ok(()));
}
