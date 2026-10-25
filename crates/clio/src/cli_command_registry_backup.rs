// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the static records of the six `backup`
//! commands: `backup create`, `backup dir`, `backup list`, `backup prune`,
//! `backup restore`, and `backup verify`.
//!
//! ## Owns
//! - The six `CommandSpec` records and the tranche that lists them in capture
//!   order, each carrying the bytes its authored usage row and catalog row
//!   held
//! - The `backup restore` and `backup verify` usage overrides: the two
//!   whole-command alternations (`ID | --latest`) that the positional-plus-
//!   flags grammar cannot write
//!
//! ## Does not own
//! - The record model or its rendering (see `cli_command_spec`)
//! - The registry and the shared global flags (see `cli_command_registry`)
//! - The backup command semantics (see `backup_cli` and the `backup_*_cli`
//!   leaf modules)
//!
//! ## Boundary
//!
//! Changes to one of these six commands' flags, usage text, or summary should
//! normally require changes here. Changes to backup semantics should not.
//!
//! Keep this module focused on the six backup records.
//
// Each record carries the bytes its authored rows held: the summary is the
// catalog row, the usage line is the registered usage text, and the notes are
// the prose the command's own `--help` printed, so both rendered surfaces and
// the frozen golden capture keep their exact output.

use crate::cli_command_registry::{GLOBAL_BACKEND, GLOBAL_DB, GLOBAL_OUTPUT};
use crate::cli_command_spec::{CommandSpec, Example, Flag, Positional, Section};

/// The globals a backup leaf accepts without printing: the shared parser
/// merges every declared global, so the leaves that print only their own flags
/// and `--output` list the other five here.
const BACKUP_UNPRINTED: &[&str] = &["db", "backend", "bank", "actor", "no-input", "yes"];

/// The globals `backup create` accepts without printing: its line already
/// documents `--db`, `--backend`, and `--output`.
const CREATE_UNPRINTED: &[&str] = &["bank", "actor", "no-input", "yes"];

/// The globals `backup restore` accepts without printing: its line already
/// documents `--yes`, `--no-input`, and `--output`.
const RESTORE_UNPRINTED: &[&str] = &["db", "backend", "bank", "actor"];

/// The globals `backup list` and `backup verify` accept without printing:
/// both read archives, never a store, so `--db` and `--backend` are refused
/// at runtime and the other four print here.
const LIST_VERIFY_UNPRINTED: &[&str] = &["bank", "actor", "no-input", "yes"];

/// The required `ID` positional of `restore` and `verify`.
const ID: Positional = Positional {
    label: "ID",
    required: true,
    description: "Snapshot id from the backup directory's listing; `restore` restores that \
                  archive and `verify` re-checks it.",
};

/// The `--keep` count: `prune` requires it and refuses a missing one, so the
/// usage line writes it bare.
const KEEP: Flag = Flag {
    required: true,
    ..Flag::uint(
        "keep",
        "N",
        "Counts the newest snapshots `backup_prune_cli::required_keep` keeps; it refuses `0` and a value that is not a count.",
    )
};

/// The `create` note: what each backend's snapshot is, and the `--keep` prune.
const CREATE_NOTE: &str = "a SQLite store is read once into a compacted, transactionally consistent copy (one VACUUM INTO, logically equivalent, never a byte-for-byte clone, and it does not retain deleted pages); a Postgres store is read in one repeatable-read transaction into a row-level dump in COPY text format, whose schema is owned by this build, so restoring it needs a target this build created. Either way the archive holds key material when a key-store file exists, so keep it owner-only. --dry-run writes nothing. --keep N then runs `clio backup prune --keep N` over the backup directory, with the same rule: without --confirm it reports what it would delete and deletes nothing";

/// The `list` note: manifests only, and the refused store flags.
const LIST_NOTE: &str = "reads each archive's manifest only: no payload, no database, so it works with the store down. Every row states its backend. --db and --backend are refused because they cannot change the listing";

/// The `prune` note: the keep rule, the never-deleted cases, and the exit codes.
const PRUNE_NOTE: &str = "keeps the newest N snapshots by manifest time and deletes the rest; the newest snapshot is never a candidate and --keep 0 is refused. An archive whose manifest cannot be read is reported as unreadable and never deleted, and a symlinked archive is reported and skipped, never followed. Without --confirm (or with --dry-run) nothing is removed and the run is a report that exits 0. Exit codes: 0 report or complete prune, 2 usage refusal, 3 a confirmed prune that could not remove every candidate";

/// The `restore` note: what is replaced, the pre-restore snapshot and the
/// swap window, the key member, and the confirmation rule.
const RESTORE_NOTE: &str = "replaces the selected store with the archive's, never a merge: the archive's database and key store overwrite the live ones, a SQLite restore swaps the archived database file in, and a Postgres restore replaces every row of the target in one transaction, so a failure halfway leaves the target untouched. A pre-restore snapshot of the current store is taken first by default; --no-pre-backup skips it and says so. The swap of a SQLite store clears the target's write-ahead log before it renames the new store in, so a crash in that window loses whatever the log had not yet written into the store; a Postgres restore replaces the target in one transaction and has no such window; that snapshot is the only thing that covers it, so --no-pre-backup accepts the window rather than removing it. The archive holds key material when it carries a key member, so keep it owner-only; --allow-missing-keys accepts an archive without one and the restored content is then permanently unreadable. On a terminal the snapshot id must be typed exactly; off one, --yes is required, and --no-input forces the non-interactive path";

/// The `verify` note: the archive-only read and the refused store flags.
const VERIFY_NOTE: &str = "reads the archive, not a store, so --db and --backend are refused because they cannot change the verdict; it re-reads the payload and the key member, compares both with the manifest, and asks each backend's own payload check whether the payload is valid";

/// The `backup restore` line, pinned whole: the two selectors are whole
/// command forms (`ID`, or `--latest`), and an alternation between two full
/// command shapes is not expressible as one positional plus one flag list, so
/// the assembled order cannot write it.
const RESTORE_USAGE: &str = "clio backup restore ID | clio backup restore --latest [--yes] [--no-input] [--no-pre-backup] [--allow-missing-keys] [--dry-run] [--output json|text]";

/// The `backup verify` line, pinned whole: the same two whole-command
/// selectors as `restore`, for the same reason the assembled order cannot
/// write an alternation between two full command shapes.
const VERIFY_USAGE: &str =
    "clio backup verify ID | clio backup verify --latest [--output json|text]";

/// `clio backup create`: snapshot one store into the backup directory.
pub(crate) const BACKUP_CREATE: CommandSpec = CommandSpec {
    name: "backup create",
    aliases: &[],
    summary: "Snapshot the store into one owner-only archive: a SQLite store is a compacted, transactionally consistent copy (one VACUUM INTO, logically equivalent, never a byte-for-byte clone, and it does not retain deleted pages); a Postgres store is a row-level dump in COPY text format, whose schema is owned by this build; --keep N then prunes the backup directory to the newest N (nothing without --confirm); exit 3 means pruning could not remove every candidate",
    notes: &[CREATE_NOTE],
    positionals: &[],
    documented: &[
        GLOBAL_DB,
        GLOBAL_BACKEND,
        Flag::text(
            "label",
            "TEXT",
            "Names the snapshot in its manifest; `backup_create_cli::take_snapshot` stores it with the archive.",
        ),
        Flag::text(
            "note",
            "TEXT",
            "Records an operator note in the archive's manifest; `backup_create_cli::take_snapshot` stores it.",
        ),
        Flag::uint(
            "keep",
            "N",
            "Runs `clio backup prune --keep N` after a successful create; `backup_create_cli` refuses `0` and a value that is not a count.",
        ),
        Flag::toggle(
            "confirm",
            "Lets the `--keep` prune delete; `backup_create_cli::confirm_needs_keep` refuses it with no `--keep` because a create deletes nothing else.",
        ),
        Flag::toggle(
            "dry-run",
            "Prints the plan and writes nothing; `backup_create_cli::dry_run` reports the sizes and the existing snapshot count.",
        ),
        GLOBAL_OUTPUT,
    ],
    accepted_extra: CREATE_UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio backup create --db ./memory.db --label nightly",
            comment: Some("Snapshot the SQLite store under a manifest label."),
        },
        Example {
            command: "clio backup create --db ./memory.db --dry-run",
            comment: Some("Report the plan and write nothing."),
        },
        Example {
            command: "clio backup create --db ./memory.db --keep 3 --confirm",
            comment: Some("Snapshot, then prune to the newest three."),
        },
    ],
    tool: None,
    section: Section::Write,
};

/// `clio backup dir`: show and create the backup directory.
pub(crate) const BACKUP_DIR: CommandSpec = CommandSpec {
    name: "backup dir",
    aliases: &[],
    summary: "Show and create the backup directory (no database needed)",
    notes: &[],
    positionals: &[],
    documented: &[GLOBAL_OUTPUT],
    accepted_extra: BACKUP_UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio backup dir",
            comment: Some("Show and create the backup directory."),
        },
        Example {
            command: "clio backup dir -o json",
            comment: Some("Print the directory path as JSON."),
        },
    ],
    tool: None,
    section: Section::Read,
};

/// `clio backup list`: every snapshot in the backup directory, newest first.
pub(crate) const BACKUP_LIST: CommandSpec = CommandSpec {
    name: "backup list",
    aliases: &[],
    summary: "List snapshots newest first by manifest time (no database needed); --db and --backend are refused because they cannot change the listing",
    notes: &[LIST_NOTE],
    positionals: &[],
    documented: &[
        Flag::uint(
            "limit",
            "N",
            "Caps the rows returned; `backup_list_cli::limit` refuses a value that is not a count.",
        ),
        GLOBAL_OUTPUT,
    ],
    accepted_extra: LIST_VERIFY_UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio backup list",
            comment: Some("List every snapshot, newest first."),
        },
        Example {
            command: "clio backup list --limit 5",
            comment: Some("Cap the listing at five snapshots."),
        },
        Example {
            command: "clio backup list -o json",
            comment: Some("Emit the rows as JSON."),
        },
    ],
    tool: None,
    section: Section::Read,
};

/// `clio backup prune`: keep the newest N snapshots, delete the rest.
pub(crate) const BACKUP_PRUNE: CommandSpec = CommandSpec {
    name: "backup prune",
    aliases: &[],
    summary: "Delete all but the newest N snapshots by manifest time: the newest snapshot is never deleted, an unreadable archive is reported and kept, and without --confirm nothing is removed. Exit codes: 0 report-only or complete prune, 3 confirmed prune could not remove every candidate",
    notes: &[PRUNE_NOTE],
    positionals: &[],
    documented: &[
        KEEP,
        Flag::toggle(
            "confirm",
            "Tells `backup_prune_cli::removes` to delete the selected candidates; absent, the run reports and removes nothing.",
        ),
        Flag::toggle(
            "dry-run",
            "Forces the report path in `backup_prune_cli::removes` even with `--confirm`, so nothing is removed.",
        ),
        GLOBAL_OUTPUT,
    ],
    accepted_extra: BACKUP_UNPRINTED,
    flag_aliases: &[],
    usage_override: None,
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio backup prune --keep 3",
            comment: Some("Report which snapshots a prune would delete."),
        },
        Example {
            command: "clio backup prune --keep 3 --confirm",
            comment: Some("Delete all but the newest three."),
        },
        Example {
            command: "clio backup prune --keep 3 --dry-run -o json",
            comment: Some("Report as JSON and remove nothing."),
        },
    ],
    tool: None,
    section: Section::Write,
};

/// `clio backup restore`: replace the selected store with the archive's.
pub(crate) const BACKUP_RESTORE: CommandSpec = CommandSpec {
    name: "backup restore",
    aliases: &[],
    summary: "Replace the selected store with the archive (never a merge); a pre-restore snapshot is taken first unless --no-pre-backup; for a SQLite store the swap removes the target's write-ahead log before it renames the new store in, so a crash in that window loses whatever that log had not yet written; a Postgres restore replaces the target in one transaction and has no such window; --no-pre-backup is what leaves that window with no way back. Exit codes: 0 clean restore, 1 operational failure, 2 refused, 3 partial or not-durable restore, 75 means the command could not report a clean success and that nothing was lost or invalid, and the command's own message says whether a retry helps: a storage lock conflict is cleared by retrying, and a restore that left a derived difference is not. Here that code is a restore that loaded every user-data row and differs in a table the store rebuilds; retrying it reloads the same archive and reproduces the same count, and the index pipeline reconciles it on its next pass",
    notes: &[RESTORE_NOTE],
    positionals: &[ID],
    documented: &[
        Flag::toggle(
            "latest",
            "Selects the newest snapshot instead of a positional id; `backup_cli::resolve_restore_id` resolves it.",
        ),
        Flag::toggle(
            "yes",
            "Answers the confirmation gate in `backup_restore_cli::confirm`, restoring without a typed snapshot id.",
        ),
        Flag::toggle(
            "no-input",
            "Forces the non-interactive path in `backup_restore_cli::confirm`, refusing rather than prompting.",
        ),
        Flag::toggle(
            "no-pre-backup",
            "Skips the pre-restore snapshot the restore backends take by default; that snapshot is the only way back.",
        ),
        Flag::toggle(
            "allow-missing-keys",
            "Accepts an archive with no key member; `backup_restore_flow::refuse_missing_keys` then restores content that stays permanently unreadable.",
        ),
        Flag::toggle(
            "dry-run",
            "Prints the restore plan through `backup_restore_flow::planned` and changes no store.",
        ),
        GLOBAL_OUTPUT,
    ],
    accepted_extra: RESTORE_UNPRINTED,
    flag_aliases: &[],
    usage_override: Some(RESTORE_USAGE),
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio backup restore clio-20260910T002640Z-0a1b2c3d --dry-run",
            comment: Some("Print the restore plan for one snapshot."),
        },
        Example {
            command: "clio backup restore clio-20260910T002640Z-0a1b2c3d --yes",
            comment: Some("Restore one snapshot without a typed confirmation."),
        },
        Example {
            command: "clio backup restore clio-20260910T002700Z-1b2c3d4e --yes",
            comment: Some("Restore a second listed snapshot."),
        },
    ],
    tool: None,
    section: Section::Write,
};

/// `clio backup verify`: one archive's digests and payload validity.
pub(crate) const BACKUP_VERIFY: CommandSpec = CommandSpec {
    name: "backup verify",
    aliases: &[],
    summary: "Re-check one archive's digests and its payload's validity; --db and --backend are refused because they cannot change the verdict",
    notes: &[VERIFY_NOTE],
    positionals: &[ID],
    documented: &[
        Flag::toggle(
            "latest",
            "Selects the newest listed snapshot instead of a positional id; `backup_verify_cli::resolve_id` resolves it.",
        ),
        GLOBAL_OUTPUT,
    ],
    accepted_extra: LIST_VERIFY_UNPRINTED,
    flag_aliases: &[],
    usage_override: Some(VERIFY_USAGE),
    usage_alternates: &[],
    examples: &[
        Example {
            command: "clio backup verify clio-20260910T002640Z-0a1b2c3d",
            comment: Some("Re-check one snapshot's digests and payload."),
        },
        Example {
            command: "clio backup verify clio-20260910T002700Z-1b2c3d4e",
            comment: Some("Verify a second listed snapshot."),
        },
        Example {
            command: "clio backup verify clio-20260910T002640Z-0a1b2c3d -o json",
            comment: Some("Verify one snapshot and print the verdict as JSON."),
        },
    ],
    tool: None,
    section: Section::Read,
};

/// The six backup records, in capture order.
pub(crate) static BACKUP_LEAVES: &[CommandSpec] = &[
    BACKUP_CREATE,
    BACKUP_DIR,
    BACKUP_LIST,
    BACKUP_PRUNE,
    BACKUP_RESTORE,
    BACKUP_VERIFY,
];

#[cfg(test)]
#[path = "cli_command_registry_backup_tests.rs"]
mod tests;
