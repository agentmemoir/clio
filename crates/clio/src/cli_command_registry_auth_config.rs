// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the seven `auth token`
//! and `config` commands: `auth token add`, `auth token list`,
//! `auth token revoke`, `config get`, `config set`, `config profiles`, and
//! `config use`.
//!
//! ## Owns
//! - The seven `CommandSpec` records and the tranche that lists them in capture
//!   order, each carrying the bytes its authored usage row and catalog row held
//! - The `auth token add` usage override: the required-then-optional `--bank`
//!   display pair the assembled order cannot write while the parser keeps
//!   collecting every occurrence
//! - The `config` group's verb keys, taken from the records' own names so the
//!   dispatcher has no second list to drift
//!
//! ## Does not own
//! - The record model or its rendering (see `cli_command_spec`)
//! - The registry and the shared global flags (see `cli_command_registry`)
//! - Token-store semantics (see `auth_cli`) or config semantics (see
//!   `config_cli`)
//!
//! ## Boundary
//!
//! Changes to one of these seven commands' flags, usage text, or summary should
//! normally require changes here. Changes to token-store or config semantics
//! should not.
//!
//! Keep this module focused on the seven auth and config records.
//
// Each record carries the bytes its authored rows held: the summary is the
// catalog row, the usage line is the registered usage text, and the notes are
// the prose the command's own `--help` printed, so both rendered surfaces and
// the frozen golden capture keep their exact output.

use crate::cli_command_registry::GLOBAL_OUTPUT;
use crate::cli_command_spec::{CommandSpec, Flag, Positional, Section};

/// The globals an auth or config leaf accepts without printing: the shared
/// parser merges every declared global, so the leaves that print only their own
/// flags and `--output` list the other six here.
const UNPRINTED: &[&str] = &["db", "backend", "bank", "actor", "no-input", "yes"];

/// The globals `auth token add` accepts without printing: its line already
/// documents `--actor` and `--bank` (its own), so the other four are listed.
const ADD_UNPRINTED: &[&str] = &["db", "backend", "no-input", "yes"];

/// The required `--label LABEL` of `add` and `revoke`: the usage lines write it
/// bare because the verbs refuse to run without one.
const LABEL: Flag = Flag {
    required: true,
    ..Flag::value("label", "LABEL")
};

/// The required `--actor ACTOR` of `add`.
const ACTOR: Flag = Flag {
    required: true,
    ..Flag::value("actor", "ACTOR")
};

/// The required `--bank BANK` occurrence of `add`: one bank per occurrence and
/// the parser collects every one, so the entry carries the repeatable mark.
const BANK_REQUIRED: Flag = Flag {
    required: true,
    repeatable: true,
    ..Flag::value("bank", "BANK")
};

/// The optional `--bank` continuation entry: the same one accepted name's
/// further occurrences, displayed as the optional form.
const BANK_OPTIONAL: Flag = Flag::value("bank", "BANK");

/// The optional `PATH` positional of `config get`.
const PATH_OPTIONAL: Positional = Positional {
    label: "PATH",
    required: false,
    description: "",
};

/// The required `PATH` positional of `config set`.
const PATH: Positional = Positional {
    label: "PATH",
    required: true,
    description: "",
};

/// The required `VALUE` positional of `config set`.
const VALUE: Positional = Positional {
    label: "VALUE",
    required: true,
    description: "",
};

/// The required `NAME` positional of `config use`.
const NAME: Positional = Positional {
    label: "NAME",
    required: true,
    description: "",
};

/// The `add` note: the one-time secret print, the store lock, and the refused
/// flags, exactly as the command's own `--help` prints them.
const ADD_NOTE: &str = "generates a credential, prints its secret exactly once, and stores only the digest in the owner-only token store; the secret cannot be recovered — save it when you see it; the store file is held under a lock for the whole write, so a run that cannot take the lock refuses without printing a secret, and --db, --backend, and --bank are refused because they cannot change what is written";

/// The `list` note: what the listing shows and the refused flags.
const LIST_NOTE: &str = "prints labels, actors, banks, and creation times; never a secret and never a digest; --db, --backend, --actor, and --bank are refused because they cannot change the listing";

/// The `revoke` note: when the change applies, the lock rule, and the refused
/// flags.
const REVOKE_NOTE: &str = "removes the record; servers load the store at start, so the change takes effect at the next server start; the write holds the store lock, so it never drops a record a concurrent add wrote, and --db, --backend, --actor, and --bank are refused";

/// The `auth token add` line, pinned whole: `--bank` is one accepted name whose
/// parser keeps every occurrence, and the assembled order writes the repeatable
/// ellipsis on every repeatable entry, so the required-then-optional display
/// pair cannot be assembled without changing the printed bytes.
const ADD_USAGE: &str = "clio auth token add --label LABEL --actor ACTOR --bank BANK [--bank BANK] [--output json|text]";

/// The `config set` scope: one of three named scopes.
const SCOPE: Flag = Flag::value("scope", "session|profile|deployment");

/// `clio auth token add`: mint a credential, print its secret once.
pub(crate) const AUTH_TOKEN_ADD: CommandSpec = CommandSpec {
    name: "auth token add",
    aliases: &[],
    summary: "Generate a network credential, print its secret exactly once, and store only the digest in the owner-only token store; the secret cannot be recovered",
    notes: &[ADD_NOTE],
    positionals: &[],
    documented: &[LABEL, ACTOR, BANK_REQUIRED, BANK_OPTIONAL, GLOBAL_OUTPUT],
    accepted_extra: ADD_UNPRINTED,
    flag_aliases: &[],
    usage_override: Some(ADD_USAGE),
    usage_alternates: &[],
    examples: &[],
    tool: None,
    section: Section::Write,
};

/// `clio auth token list`: the store's records, never a secret or a digest.
pub(crate) const AUTH_TOKEN_LIST: CommandSpec = CommandSpec {
    name: "auth token list",
    aliases: &[],
    summary: "Show labels, actors, banks, and created_at; never a secret or a digest",
    notes: &[LIST_NOTE],
    positionals: &[],
    documented: &[GLOBAL_OUTPUT],
    accepted_extra: UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: None,
    section: Section::Read,
};

/// `clio auth token revoke`: remove one credential record.
pub(crate) const AUTH_TOKEN_REVOKE: CommandSpec = CommandSpec {
    name: "auth token revoke",
    aliases: &[],
    summary: "Remove the record; takes effect at the next server start",
    notes: &[REVOKE_NOTE],
    positionals: &[],
    documented: &[LABEL, GLOBAL_OUTPUT],
    accepted_extra: UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: None,
    section: Section::Write,
};

/// `clio config get`: the effective configuration, or one dotted path.
pub(crate) const CONFIG_GET: CommandSpec = CommandSpec {
    name: "config get",
    aliases: &[],
    summary: "read one configuration path",
    notes: &[],
    positionals: &[PATH_OPTIONAL],
    documented: &[GLOBAL_OUTPUT],
    accepted_extra: UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("config_get"),
    section: Section::Read,
};

/// `clio config set`: write one configuration path at a scope.
pub(crate) const CONFIG_SET: CommandSpec = CommandSpec {
    name: "config set",
    aliases: &[],
    summary: "write one configuration path at a scope",
    notes: &[],
    positionals: &[PATH, VALUE],
    documented: &[SCOPE, GLOBAL_OUTPUT],
    accepted_extra: UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("config_set"),
    section: Section::Write,
};

/// `clio config profiles`: the named profiles.
pub(crate) const CONFIG_PROFILES: CommandSpec = CommandSpec {
    name: "config profiles",
    aliases: &[],
    summary: "list the available configuration profiles",
    notes: &[],
    positionals: &[],
    documented: &[GLOBAL_OUTPUT],
    accepted_extra: UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("config_profiles"),
    section: Section::Read,
};

/// `clio config use`: apply one profile by name.
pub(crate) const CONFIG_USE: CommandSpec = CommandSpec {
    name: "config use",
    aliases: &[],
    summary: "apply one profile by name",
    notes: &[],
    positionals: &[NAME],
    documented: &[GLOBAL_OUTPUT],
    accepted_extra: UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[],
    tool: Some("config_profile_apply"),
    section: Section::Write,
};

/// The `config` group's verb keys: the four config records' own names, in
/// tranche order. `RuntimeGroup::verbs` needs a static slice, so the names are
/// taken from the records rather than retyped; the tranche tests pin this list
/// equal to the prefix-derived registry list.
pub(crate) static CONFIG_VERBS: &[&str] = &[
    CONFIG_GET.name,
    CONFIG_SET.name,
    CONFIG_PROFILES.name,
    CONFIG_USE.name,
];

/// The seven auth and config records, in capture order.
pub(crate) static AUTH_CONFIG_LEAVES: &[CommandSpec] = &[
    AUTH_TOKEN_ADD,
    AUTH_TOKEN_LIST,
    AUTH_TOKEN_REVOKE,
    CONFIG_GET,
    CONFIG_SET,
    CONFIG_PROFILES,
    CONFIG_USE,
];

#[cfg(test)]
#[path = "cli_command_registry_auth_config_tests.rs"]
mod tests;
