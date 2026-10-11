// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the six backup records against the
//! usage bytes and parser flag sets the commands print today, and for pinning
//! the two whole-command overrides and the three `or:` alternates.
//!
//! ## Owns
//! - Byte equality of each record's `full_usage` and `usage_line` with the
//!   registered usage text
//! - The exact `parser_spec` value and boolean flag sequences per command
//! - Each record's summary, tool, section, positional, and required-flag values
//! - The override pin: `restore` and `verify` are the tranche's only pinned
//!   lines, and each carries one whole-command alternation
//! - The `or:` pin: the whole registry holds exactly three alternates, and
//!   each renders its registered line
//!
//! ## Does not own
//! - The registry lookups and validation (`cli_command_registry_tests`)
//! - The frozen golden capture (`golden_repeatability_test`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the six backup records' data.

use crate::cli_command_registry_test_support::record;
use crate::cli_command_spec::{Positional, Section, ValueKind};

/// `backup create` renders today's usage text and builds today's parser sets.
#[test]
fn backup_create_matches_the_registered_usage_and_parser() {
    let spec = record("backup create");
    assert_eq!(
        spec.full_usage(),
        "usage: clio backup create [--db PATH] [--backend sqlite|postgres] [--label TEXT] \
         [--note TEXT] [--keep N] [--confirm] [--dry-run] [--output json|text]\n       \
         a SQLite store is read once into a compacted, transactionally consistent copy (one \
         VACUUM INTO, logically equivalent, never a byte-for-byte clone, and it does not retain \
         deleted pages); a Postgres store is read in one repeatable-read transaction into a \
         row-level dump in COPY text format, whose schema is owned by this build, so restoring \
         it needs a target this build created. Either way the archive holds key material when a \
         key-store file exists, so keep it owner-only. --dry-run writes nothing. --keep N then \
         runs `clio backup prune --keep N` over the backup directory, with the same rule: \
         without --confirm it reports what it would delete and deletes nothing",
        "backup create full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio backup create [--db PATH] [--backend sqlite|postgres] [--label TEXT] \
         [--note TEXT] [--keep N] [--confirm] [--dry-run] [--output json|text]",
        "backup create usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        [
            "db", "backend", "bank", "actor", "output", "label", "note", "keep"
        ]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "confirm", "dry-run"]
    );
    assert_eq!(spec.tool, None);
    assert_eq!(spec.section, Section::Write);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// `backup dir` renders today's usage text and builds today's parser sets.
#[test]
fn backup_dir_matches_the_registered_usage_and_parser() {
    let spec = record("backup dir");
    assert_eq!(
        spec.full_usage(),
        "usage: clio backup dir [--output json|text]",
        "backup dir full usage"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.tool, None);
    assert_eq!(spec.section, Section::Read);
    assert!(spec.notes.is_empty(), "backup dir notes");
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// `backup list` renders today's usage text and builds today's parser sets.
#[test]
fn backup_list_matches_the_registered_usage_and_parser() {
    let spec = record("backup list");
    assert_eq!(
        spec.full_usage(),
        "usage: clio backup list [--limit N] [--output json|text]\n       \
         reads each archive's manifest only: no payload, no database, so it works with the \
         store down. Every row states its backend. --db and --backend are refused because \
         they cannot change the listing",
        "backup list full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio backup list [--limit N] [--output json|text]",
        "backup list usage line"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "limit"]
    );
    assert_eq!(spec.parser_spec().bool_flags(), ["no-input", "yes"]);
    assert_eq!(spec.tool, None);
    assert_eq!(spec.section, Section::Read);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// `backup prune` renders today's usage text and builds today's parser sets;
/// `--keep` is the one flag the line writes bare.
#[test]
fn backup_prune_matches_the_registered_usage_and_parser() {
    let spec = record("backup prune");
    assert_eq!(
        spec.full_usage(),
        "usage: clio backup prune --keep N [--confirm] [--dry-run] [--output json|text]\n       \
         keeps the newest N snapshots by manifest time and deletes the rest; the newest \
         snapshot is never a candidate and --keep 0 is refused. An archive whose manifest \
         cannot be read is reported as unreadable and never deleted, and a symlinked archive \
         is reported and skipped, never followed. Without --confirm (or with --dry-run) \
         nothing is removed and the run is a report that exits 0. Exit codes: 0 report or \
         complete prune, 2 usage refusal, 3 a confirmed prune that could not remove every \
         candidate",
        "backup prune full usage"
    );
    assert_eq!(
        spec.usage_line(),
        "clio backup prune --keep N [--confirm] [--dry-run] [--output json|text]",
        "backup prune usage line"
    );
    assert_eq!(spec.required_flags(), ["keep"], "the bare flag");
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output", "keep"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "confirm", "dry-run"]
    );
    assert_eq!(spec.tool, None);
    assert_eq!(spec.section, Section::Write);
    assert!(spec.positionals.is_empty(), "{:?}", spec.positionals);
}

/// `backup restore` renders today's pinned line and note, and its two
/// selectors are whole command forms joined by `|`, which the assembled order
/// cannot write: that is the recorded reason the line is an override.
#[test]
fn backup_restore_matches_the_registered_usage_and_parser() {
    let spec = record("backup restore");
    assert_eq!(
        spec.full_usage(),
        "usage: clio backup restore ID | clio backup restore --latest [--yes] [--no-input] \
         [--no-pre-backup] [--allow-missing-keys] [--dry-run] [--output json|text]\n       \
         replaces the selected store with the archive's, never a merge: the archive's database \
         and key store overwrite the live ones, a SQLite restore swaps the archived database \
         file in, and a Postgres restore replaces every row of the target in one transaction, \
         so a failure halfway leaves the target untouched. A pre-restore snapshot of the \
         current store is taken first by default; --no-pre-backup skips it and says so. The \
         swap of a SQLite store clears the target's write-ahead log before it renames the new \
         store in, so a crash in that window loses whatever the log had not yet written into \
         the store; a Postgres restore replaces the target in one transaction and has no such \
         window; that snapshot is the only thing that covers it, so --no-pre-backup accepts \
         the window rather than removing it. The archive holds key material when it carries a \
         key member, so keep it owner-only; --allow-missing-keys accepts an archive without \
         one and the restored content is then permanently unreadable. On a terminal the \
         snapshot id must be typed exactly; off one, --yes is required, and --no-input forces \
         the non-interactive path",
        "backup restore full usage"
    );
    let pinned = spec.usage_override.expect("the restore line is pinned");
    let (id_form, latest_form) = pinned.split_once(" | ").expect("two alternations");
    assert!(id_form.starts_with("clio backup restore "), "{id_form}");
    assert!(
        latest_form.starts_with("clio backup restore "),
        "{latest_form}"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        [
            "no-input",
            "yes",
            "latest",
            "no-pre-backup",
            "allow-missing-keys",
            "dry-run"
        ]
    );
    assert_eq!(spec.tool, None);
    assert_eq!(spec.section, Section::Write);
    assert_eq!(
        spec.positionals,
        [Positional {
            label: "ID",
            required: true,
            description: ""
        }]
        .as_slice()
    );
}

/// `backup verify` renders today's pinned line and note, with the same
/// whole-command alternation shape as `restore`.
#[test]
fn backup_verify_matches_the_registered_usage_and_parser() {
    let spec = record("backup verify");
    assert_eq!(
        spec.full_usage(),
        "usage: clio backup verify ID | clio backup verify --latest [--output json|text]\n       \
         reads the archive, not a store, so --db and --backend are refused because they \
         cannot change the verdict; it re-reads the payload and the key member, compares both \
         with the manifest, and asks each backend's own payload check whether the payload is \
         valid",
        "backup verify full usage"
    );
    let pinned = spec.usage_override.expect("the verify line is pinned");
    let (id_form, latest_form) = pinned.split_once(" | ").expect("two alternations");
    assert!(id_form.starts_with("clio backup verify "), "{id_form}");
    assert!(
        latest_form.starts_with("clio backup verify "),
        "{latest_form}"
    );
    assert_eq!(
        spec.parser_spec().value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(
        spec.parser_spec().bool_flags(),
        ["no-input", "yes", "latest"]
    );
    assert_eq!(spec.tool, None);
    assert_eq!(spec.section, Section::Read);
    assert_eq!(
        spec.positionals,
        [Positional {
            label: "ID",
            required: true,
            description: ""
        }]
        .as_slice()
    );
}

/// The two overrides are the tranche's only pinned lines; the other four
/// records assemble their lines from positionals and flags, so a flag added to
/// a record shows on its line with no edit here.
#[test]
fn restore_and_verify_are_the_only_overridden_lines() {
    for name in ["backup create", "backup dir", "backup list", "backup prune"] {
        assert!(
            record(name).usage_override.is_none(),
            "{name} derives its line"
        );
        assert!(
            record(name).usage_alternates.is_empty(),
            "{name} alternates"
        );
    }
    for (name, summary) in [
        (
            "backup create",
            "Snapshot the store into one owner-only archive: a SQLite store is a compacted, \
             transactionally consistent copy (one VACUUM INTO, logically equivalent, never a \
             byte-for-byte clone, and it does not retain deleted pages); a Postgres store is a \
             row-level dump in COPY text format, whose schema is owned by this build; --keep N \
             then prunes the backup directory to the newest N (nothing without --confirm); \
             exit 3 means pruning could not remove every candidate",
        ),
        (
            "backup dir",
            "Show and create the backup directory (no database needed)",
        ),
        (
            "backup list",
            "List snapshots newest first by manifest time (no database needed); --db and \
             --backend are refused because they cannot change the listing",
        ),
        (
            "backup prune",
            "Delete all but the newest N snapshots by manifest time: the newest snapshot is \
             never deleted, an unreadable archive is reported and kept, and without --confirm \
             nothing is removed. Exit codes: 0 report-only or complete prune, 3 confirmed \
             prune could not remove every candidate",
        ),
        (
            "backup restore",
            "Replace the selected store with the archive (never a merge); a pre-restore \
             snapshot is taken first unless --no-pre-backup; for a SQLite store the swap \
             removes the target's write-ahead log before it renames the new store in, so a \
             crash in that window loses whatever that log had not yet written; a Postgres \
             restore replaces the target in one transaction and has no such window; \
             --no-pre-backup is what leaves that window with no way back. Exit codes: 0 clean \
             restore, 1 operational failure, 2 refused, 3 partial or not-durable restore",
        ),
        (
            "backup verify",
            "Re-check one archive's digests and its payload's validity; --db and --backend \
             are refused because they cannot change the verdict",
        ),
    ] {
        assert_eq!(record(name).summary, summary, "{name} summary");
    }
}

/// The whole registry holds exactly three `or:` alternates — `admit`,
/// `batch`, and `hygiene clean` — and each renders the registered line after
/// `usage:`. A fourth alternate, or a reworded one, fails here instead of
/// drifting past the frozen capture.
#[test]
fn the_registry_holds_exactly_three_or_alternates_and_they_render() {
    let mut with_alternates: Vec<&str> = crate::cli_command_registry::all()
        .filter(|spec| !spec.usage_alternates.is_empty())
        .map(|spec| spec.name)
        .collect();
    with_alternates.sort_unstable();
    assert_eq!(
        with_alternates,
        ["admit", "batch", "hygiene clean"],
        "the alternates' owners changed"
    );
    for (name, alternate) in [
        (
            "admit",
            "clio admit --file ops.json [--bank BANK] [--actor ACTOR]",
        ),
        (
            "batch",
            "clio batch --file - (reads operations JSON from stdin)",
        ),
        (
            "hygiene clean",
            "clio hygiene clean --file - (reads the candidates JSON from stdin)",
        ),
    ] {
        let spec = record(name);
        assert_eq!(spec.usage_alternates, [alternate], "{name} alternate");
        let rendered = format!("\n       or: {alternate}");
        assert!(
            spec.full_usage().contains(&rendered),
            "{name} must render its or: line: {}",
            spec.full_usage()
        );
    }
}

/// The six records bind no MCP tool and declare no aliases, flag aliases, or
/// examples; `dir` is the one leaf with no note.
#[test]
fn the_six_records_carry_no_extra_surfaces() {
    for name in [
        "backup create",
        "backup dir",
        "backup list",
        "backup prune",
        "backup restore",
        "backup verify",
    ] {
        let spec = record(name);
        assert!(spec.aliases.is_empty(), "{name} aliases");
        assert!(spec.flag_aliases.is_empty(), "{name} flag aliases");
        assert!(spec.usage_alternates.is_empty(), "{name} alternates");
        assert!(spec.examples.is_empty(), "{name} examples");
        assert_eq!(spec.tool, None, "{name} tool");
    }
    assert_eq!(record("backup dir").notes.len(), 0, "backup dir notes");
    for name in [
        "backup create",
        "backup list",
        "backup prune",
        "backup restore",
        "backup verify",
    ] {
        assert_eq!(record(name).notes.len(), 1, "{name} notes");
    }
}

/// The real registry satisfies `validate` against the catalog inventory.
#[test]
fn validate_accepts_the_backup_records_against_the_catalog_inventory() {
    let specs: Vec<&crate::cli_command_spec::CommandSpec> =
        crate::cli_command_registry::all().collect();
    let inventory: Vec<&str> = crate::command_catalog::catalog_commands().collect();
    assert_eq!(
        crate::cli_command_registry::validate(&specs, &inventory),
        Ok(())
    );
}

/// Every documented flag in the six backup records carries complete metadata:
/// a description, and for a value flag a placeholder and a non-empty kind.
#[test]
fn metadata_check_accepts_the_backup_records() {
    let specs: Vec<&crate::cli_command_spec::CommandSpec> =
        crate::cli_command_registry_backup::BACKUP_LEAVES
            .iter()
            .collect();
    assert_eq!(crate::cli_command_registry::check_metadata(&specs), Ok(()));
}

/// Each backup value flag carries the kind its named consumer parses: the
/// counts are whole numbers, and the two labels stay free text.
#[test]
fn the_backup_value_flags_carry_the_kind_their_consumer_parses() {
    let value = |name: &str, flag: &str| {
        record(name)
            .documented
            .iter()
            .find(|candidate| candidate.long == flag)
            .unwrap_or_else(|| panic!("`{name}` documents `--{flag}`"))
            .value
            .unwrap_or_else(|| panic!("`{name} --{flag}` takes a value"))
    };
    for (name, flag, kind) in [
        ("backup create", "label", ValueKind::Text),
        ("backup create", "note", ValueKind::Text),
        ("backup create", "keep", ValueKind::Uint),
        ("backup list", "limit", ValueKind::Uint),
        ("backup prune", "keep", ValueKind::Uint),
    ] {
        assert_eq!(value(name, flag).kind, kind, "`{name} --{flag}`");
    }
}
