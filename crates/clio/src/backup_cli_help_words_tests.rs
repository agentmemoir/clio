// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the backend-neutral wording the `clio backup` help
//! surfaces carry, that the `clio help` catalog cannot contradict the
//! per-subcommand `--help` text an operator reads beside it, and that a refusal
//! hint names no directory a run of the other backend does not have.
//!
//! ## Owns
//! - One assertion per backend-neutral phrase `backup create/list/restore/
//!   verify` must carry: a SQLite snapshot is not byte-identical and keeps no
//!   deleted pages, a Postgres snapshot is a row-level dump in COPY text
//!   format whose schema this build owns, a restore replaces every row, a
//!   listing states each row's backend, a verify names each backend's own check
//! - The `clio help` catalog ↔ `backup create --help` parity pin
//! - The catalog's account of the swap window `--no-pre-backup` leaves uncovered
//! - That the swap window is scoped to the SQLite store, and that the staging
//!   hint names the directory the run stages into rather than a target
//!   directory a Postgres restore never has
//!
//! ## Does not own
//! - The catalog and usage strings under test (`command_catalog`, `cli_help_usage`)
//! - The refusal hint constants themselves (see `backup_restore_flow`)
//! - The per-verb behavior suites (see `backup_create_cli_tests` and friends)
//! - Help wording owned by other surfaces (`ops_cli`, for one)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Tests here break when an
//! operator-facing help claim moves; they never change the claim themselves.
//!
//! Keep this module focused on the backup help wording.

use crate::backup_cli_fixtures::{Scratch, run_backup};
use crate::*;

/// The claim both surfaces must carry, word for word, however each of them
/// wraps its lines: a SQLite snapshot is a compacted copy, never a
/// byte-for-byte clone, and it keeps no deleted pages.
///
/// The claim is checked against each surface with its line breaks collapsed to
/// single spaces, because the catalog wraps this sentence across five lines and
/// the usage line wraps it across none. The wording has to be the same either
/// way: two paraphrases of one claim are how the surfaces drifted apart before.
const SHARED_SQLITE_CLAIM: &str = "compacted, transactionally consistent copy (one VACUUM INTO, \
logically equivalent, never a byte-for-byte clone, and it does not retain deleted pages)";

/// The phrases both the `clio help` catalog and `backup create --help` carry
/// about what a snapshot is, whichever backend produced it.
///
/// These are the claims the two operator-facing surfaces used to contradict:
/// the catalog called the SQLite snapshot a "file copy" while the subcommand
/// help said it was a compacted copy and never a byte-for-byte clone.
const SHARED_SNAPSHOT_CLAIMS: &[&str] = &[
    "compacted, transactionally consistent copy",
    "never a byte-for-byte clone",
    "does not retain deleted pages",
    "row-level dump in COPY text format",
    "whose schema is owned by this build",
];

/// The Postgres half of the swap window, worded the same way on both surfaces:
/// a Postgres restore replaces the target in one transaction, so it has no
/// write-ahead log to lose and no window in which a crash costs a write.
const POSTGRES_HAS_NO_SWAP_WINDOW: &str =
    "a Postgres restore replaces the target in one transaction and has no such window";

/// The catalog's block for `command`: its command line and the wrapped
/// description lines that follow it.
///
/// The description lines are found by indentation rather than by counting
/// lines, so rewording the command line above them cannot shift the block out
/// from under the scan; the scan still stops at the next command line, so a
/// line added below them is not silently swallowed.
fn catalog_block(command: &str) -> String {
    // The rendered catalog, not a stored copy of it: the flags are rendered from
    // the usage registry, so a scan over a separate string would no longer be
    // scanning what a reader sees.
    let text = printed_help();
    let start = text
        .lines()
        .position(|line| line.trim_start().starts_with(command))
        .unwrap_or_else(|| panic!("the catalog must list `{command}`"));
    let mut block: Vec<&str> = Vec::new();
    for line in text.lines().skip(start) {
        if !block.is_empty() && line.trim_start().starts_with("clio ") {
            break;
        }
        block.push(line);
    }
    block.join("\n")
}

/// The body `clio help` prints, with the wrapping `{range}` placeholder
/// already substituted — the same function `print_help` calls.
fn printed_help() -> String {
    crate::cli_top_level::help_text()
}

/// The help text one `backup` subcommand prints for `--help`, taken from the
/// real command surface rather than from the usage table.
fn subcommand_help(label: &str, subcommand: &str) -> String {
    let scratch = Scratch::new(label);
    let (code, out, err) = run_backup(
        &[subcommand, "--help"],
        &[],
        Ok(scratch.path().to_path_buf()),
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "`{subcommand} --help`: {err}");
    out
}

/// The text with every run of whitespace collapsed to one space, so a phrase
/// that either surface wraps across lines is still matched as one string.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Assert `text` carries every one of `phrases`, naming the missing one so a
/// failure points at the wording rather than only at the surface.
///
/// The text is compared with its whitespace collapsed, so a phrase counts as
/// present wherever the surface chose to break its line.
#[track_caller]
fn assert_phrases(surface: &str, text: &str, phrases: &[&str]) {
    let flat = one_line(text);
    for phrase in phrases {
        assert!(
            flat.contains(phrase),
            "{surface} must say {phrase:?}\n--- {surface} ---\n{text}"
        );
    }
}

/// The two surfaces as `(label, text)`, so one loop can check both.
fn both_surfaces<'a>(catalog: &'a str, usage: &'a str) -> [(&'static str, &'a str); 2] {
    [("clio help", catalog), ("clio backup create --help", usage)]
}

/// `clio backup create --help`: the SQLite copy is compacted and never
/// byte-identical, and a Postgres store becomes a row-level dump in COPY text
/// format whose schema this build owns.
#[test]
fn create_help_pins_the_backend_neutral_snapshot_claims() {
    let out = subcommand_help("create-help-words", "create");

    assert_phrases("clio backup create --help", &out, SHARED_SNAPSHOT_CLAIMS);
}

/// `clio backup list --help`: every row states its own backend, so an operator
/// never has to open a store to tell the two backends' archives apart.
#[test]
fn list_help_pins_the_per_row_backend_claim() {
    let out = subcommand_help("list-help-words", "list");

    assert_phrases(
        "clio backup list --help",
        &out,
        &["Every row states its backend"],
    );
}

/// `clio backup restore --help`: a restore replaces the store, never merges,
/// and for Postgres that means every row, in one transaction.
#[test]
fn restore_help_pins_the_replaces_every_row_claim() {
    let out = subcommand_help("restore-help-words", "restore");

    assert_phrases(
        "clio backup restore --help",
        &out,
        &["replaces every row of the target in one transaction"],
    );
}

/// `clio backup verify --help`: the verdict comes from each backend's own
/// payload check, not from a digest comparison alone.
#[test]
fn verify_help_pins_the_backend_owned_check_claim() {
    let out = subcommand_help("verify-help-words", "verify");

    assert_phrases(
        "clio backup verify --help",
        &out,
        &["asks each backend's own payload check whether the payload is valid"],
    );
}

/// `clio help` and `clio backup create --help` are read by the same operator
/// minutes apart, so they may not disagree about what a snapshot is.
#[test]
fn the_help_catalog_agrees_with_the_create_help_text() {
    let catalog = printed_help();
    let usage = subcommand_help("catalog-parity-create", "create");

    // Neither surface may call the SQLite snapshot a plain file copy: the two
    // surfaces used to make that claim in opposite directions.
    for (surface, text) in both_surfaces(&catalog, &usage) {
        assert!(
            !text.contains("SQLite file copy"),
            "{surface} must not call the snapshot a file copy\n--- {surface} ---\n{text}"
        );
    }

    // Every backend-neutral claim carries in both surfaces. This fails if
    // either side drops one, and if the catalog drifts from `create --help`
    // about what a snapshot is.
    for (surface, text) in both_surfaces(&catalog, &usage) {
        assert_phrases(surface, text, SHARED_SNAPSHOT_CLAIMS);
    }
}

/// The SQLite claim the two surfaces share is one claim, not two
/// paraphrases: the catalog's wrapped description and the usage line carry the
/// same words, so neither can quietly change what a snapshot is on its own.
#[test]
fn both_snapshot_surfaces_carry_the_same_sqlite_claim() {
    let block = one_line(&catalog_block("clio backup create"));
    let usage = one_line(&subcommand_help("sqlite-claim-create", "create"));

    assert!(
        block.contains(SHARED_SQLITE_CLAIM),
        "the catalog's `clio backup create` block must carry the shared snapshot claim\n\
         --- block ---\n{block}"
    );
    assert!(
        usage.contains(SHARED_SQLITE_CLAIM),
        "`clio backup create --help` must carry the shared snapshot claim\n--- usage ---\n{usage}"
    );
}

/// `clio help` must name the one direction of the swap order that costs
/// something, and must say that `--no-pre-backup` is what leaves it with no way
/// back. An operator who reads only the catalog cannot otherwise learn the
/// window exists, and `--no-pre-backup` reads like a harmless speed-up.
///
/// The window belongs to a SQLite store: for one, the swap removes the target's
/// write-ahead log before the rename, so a crash in that window loses whatever
/// the log had not yet written into the store.
#[test]
fn the_catalog_names_the_window_no_pre_backup_leaves_uncovered() {
    let catalog = printed_help();

    assert_phrases(
        "clio help",
        &catalog,
        &[
            "for a SQLite store the swap removes the target's write-ahead log before it renames the new store in",
            "a crash in that window loses whatever that log had not yet written",
            "--no-pre-backup is what leaves that window with no way back",
        ],
    );
}

/// `clio backup restore --help` must name the same window, because that is the
/// surface an operator reaches by asking the command itself. Before this, it
/// said only that `--no-pre-backup` "skips it and says so", which reads like a
/// harmless speed-up over a window that costs un-checkpointed transactions.
#[test]
fn the_restore_subcommand_help_names_the_window_too() {
    let out = subcommand_help("restore-window-words", "restore");

    assert_phrases(
        "clio backup restore --help",
        &out,
        &[
            "The swap of a SQLite store clears the target's write-ahead log before it renames the new store in",
            "a crash in that window loses whatever the log had not yet written into the store",
            "--no-pre-backup accepts the window rather than removing it",
        ],
    );
}

/// A Postgres operator must not be told about a write-ahead log. The window
/// that loses un-checkpointed writes is the SQLite store swap, so each surface
/// an operator can reach has to name that swap for what it is and say the
/// Postgres path has no such window; otherwise a Postgres operator reads about
/// a sidecar file and a crash window that a one-transaction replace cannot
/// have.
///
/// The help entry is read through the lookup the CLI itself uses, so this
/// covers the entry `clio backup restore --help` actually prints.
#[test]
fn a_postgres_restore_is_not_told_about_a_write_ahead_log() {
    let help = crate::cli_help_usage::help_for("backup restore")
        .expect("the `backup restore` help entry the CLI resolves");
    let catalog = catalog_block("clio backup restore");

    for (surface, text, sqlite_swap) in [
        (
            "clio backup restore --help",
            help.as_str(),
            "The swap of a SQLite store clears the target's write-ahead log before it renames the new store in",
        ),
        (
            "clio help",
            catalog.as_str(),
            "for a SQLite store the swap removes the target's write-ahead log before it renames the new store in",
        ),
    ] {
        assert_phrases(surface, text, &[sqlite_swap, POSTGRES_HAS_NO_SWAP_WINDOW]);
    }
}

/// The staging-failure hint must not point an operator at the target's
/// directory. A SQLite restore stages its members in the target's own
/// directory, but a Postgres restore extracts into the backup directory and has
/// no target directory on disk, so naming the target sends that operator to
/// free space in a place the run never wrote to. The hint names the directory
/// the staged member goes in; the message body beside it names the real path.
#[test]
fn the_staging_hint_does_not_name_the_wrong_directory() {
    let hint = crate::backup_restore_flow::HINT_STAGING_FAILED;

    assert!(
        !hint.contains("the target's directory"),
        "the staging hint must not send a Postgres operator to a target directory: {hint}"
    );
    assert!(
        hint.contains("the directory the staged member goes in"),
        "the staging hint must name the directory the run stages into: {hint}"
    );
}

/// `clio help` must not name a top-level `reindex`: the real surface is
/// `clio ops reindex`, and `--confirm` is what makes it mutate. An operator
/// reading the recovery page after a Postgres restore needs the catalog to say
/// so, not a verb the dispatcher does not accept.
#[test]
fn the_catalog_names_reindex_only_under_ops_with_confirm() {
    let catalog = printed_help();

    assert!(
        !catalog
            .lines()
            .any(|line| line.trim_start().starts_with("clio reindex")),
        "`clio help` must not list a top-level `clio reindex`:\n{catalog}"
    );
    let reindex = crate::help_flags_test_support::entry_block(&catalog, "ops reindex");
    assert!(
        !reindex.is_empty(),
        "`clio help` must list `clio ops reindex`:\n{catalog}"
    );
    assert!(
        reindex.contains("[--confirm]"),
        "`clio ops reindex` must document that --confirm gates the live run: {reindex}"
    );
    // The catalog wraps to a column, so a phrase can straddle a line break; the
    // comparison collapses whitespace for the same reason the wording tests do.
    let flat = catalog.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("(live run requires --confirm)"),
        "the catalog must say that the reindex live run requires --confirm:\n{catalog}"
    );
}
