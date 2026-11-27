// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the two resolutions the `clio backup` verbs share: the
//! store a snapshot reads, and the snapshot a restore selects.
//!
//! ## Owns
//! - The accepted store shapes: a SQLite file, and a Postgres database named
//!   without its credentials
//! - The refusals: a backend this build does not snapshot, an in-memory store,
//!   and a store file a snapshot needs but a restore does not
//! - The restore selector: `--latest`, a listed id, and a missing id with and
//!   without a close neighbour to suggest
//!
//! ## Does not own
//! - The precedence chain that picks the address (`clio-config`)
//! - The flag grammar that reads `--db` and `--backend` (`cli_args`)
//! - The verb runs that call these resolutions (see `backup_cli_tests` and the
//!   per-verb suites)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Tests here break when a
//! resolution stops refusing what it refused, or reports a fact it did not
//! report before.
//!
//! Keep this module focused on store and snapshot resolution.

use super::*;
use crate::backup_cli_fixtures::tokens;
use crate::setup_support_tests::temp_dir;

/// An environment lookup that answers nothing, so no test here reads the host
/// environment. Every value a case needs arrives through `--db` or `--backend`.
fn no_env() -> impl Fn(&str) -> Option<String> {
    |_key: &str| None
}

/// Parsed flags for a verb that takes the globals `--db` and `--backend`.
fn store_flags(args: &[&str]) -> ParsedArgs {
    cli_args::parse(&FlagSpec::new(&[], &[]), tokens(args)).expect("parse the store flags")
}

/// Parsed flags for a restore's own `--latest` toggle and its id positional.
fn restore_flags(args: &[&str]) -> ParsedArgs {
    cli_args::parse(&FlagSpec::new(&[], &["latest"]), tokens(args))
        .expect("parse the restore flags")
}

/// One listing row, so a selector case needs no archive on disk. Selection is a
/// function of the rows alone, and building them here is what proves that.
fn row(id: &str) -> SnapshotRow {
    SnapshotRow {
        id: id.to_owned(),
        path: PathBuf::from(format!("/backups/{id}.cliobak.tar.gz")),
        created_at: "2026-11-01T00:00:00Z".to_owned(),
        backend: "sqlite".to_owned(),
        slug: "clio".to_owned(),
        archive_bytes: 1,
        payload_bytes: 2,
        keys_included: false,
        label: None,
        unreadable: None,
    }
}

/// A SQLite store file resolves to that file: the snapshot reads the path, the
/// report shows the path, and the key store travels beside it.
#[test]
fn a_sqlite_store_file_resolves_to_the_file_it_names() {
    let dir = temp_dir("backup-cli-target-sqlite");
    let store = dir.join("memory.db");
    std::fs::write(&store, b"sqlite bytes").expect("write the store file");
    let store_text = store.display().to_string();
    let parsed = store_flags(&["--db", &store_text]);

    let target = resolve_target(&no_env(), &parsed, "create").expect("a store file resolves");

    assert_eq!(target.backend, "sqlite");
    assert!(!target.is_postgres());
    assert_eq!(target.url, store_text);
    assert_eq!(target.path, store);
    assert_eq!(target.database, "memory.db");
    assert_eq!(target.display, store_text);
    assert_eq!(
        target.kms_path,
        Some(PathBuf::from(format!("{store_text}.kms.json")))
    );
}

/// A snapshot of a store file that is not there is refused before any work, so
/// the operator learns the file is missing instead of getting an empty copy.
#[test]
fn a_snapshot_refuses_a_store_file_that_is_not_there() {
    let dir = temp_dir("backup-cli-target-missing");
    let missing = dir.join("absent.db");
    let missing_text = missing.display().to_string();
    let parsed = store_flags(&["--db", &missing_text]);

    let error = resolve_target(&no_env(), &parsed, "create")
        .map(drop)
        .expect_err("a snapshot needs a store file to read");

    assert_eq!(error.code(), "not_found");
    assert!(error.message().contains("no store at"), "{error:?}");
    assert!(error.message().contains("absent.db"), "{error:?}");
    assert!(
        error.hint().is_some_and(|hint| hint.contains("clio setup")),
        "the hint names what would create the store: {error:?}"
    );
}

/// A restore may place the first store at a path that does not exist yet, so the
/// same missing file is accepted there and nothing is created by resolving.
#[test]
fn a_restore_may_target_a_store_file_that_is_not_there_yet() {
    let dir = temp_dir("backup-cli-target-restore");
    let missing = dir.join("first.db");
    let missing_text = missing.display().to_string();
    let parsed = store_flags(&["--db", &missing_text]);

    let target = resolve_restore_target(&no_env(), &parsed, "restore")
        .expect("a restore may place the first store");

    assert_eq!(target.path, missing);
    assert_eq!(target.backend, "sqlite");
    assert!(!target.path.exists(), "resolving creates nothing");
}

/// An in-memory store holds nothing a snapshot could read, so it is refused with
/// the hint that names the flag to change.
#[test]
fn an_in_memory_store_is_refused_with_the_file_hint() {
    let parsed = store_flags(&["--db", "sqlite::memory:"]);

    let error = resolve_target(&no_env(), &parsed, "create")
        .map(drop)
        .expect_err("an in-memory store has nothing to snapshot");

    assert_eq!(error.code(), "usage");
    assert!(error.message().contains("needs a store file"), "{error:?}");
    assert!(
        error
            .hint()
            .is_some_and(|hint| hint.contains("point `--db` at a SQLite database file")),
        "the hint names the flag and the value shape: {error:?}"
    );
}

/// A Postgres target reports the database name, never the credentials, and has
/// no store file to read.
#[test]
fn a_postgres_target_reports_the_database_name_not_the_credentials() {
    let parsed = store_flags(&["--db", "postgres://clio:s3cret@127.0.0.1:5432/clio"]);

    let target = resolve_target(&no_env(), &parsed, "create").expect("a Postgres address resolves");

    assert_eq!(target.backend, "postgres");
    assert!(target.is_postgres());
    assert_eq!(target.database, "clio");
    assert_eq!(target.display, "clio");
    assert_eq!(target.path, PathBuf::new());
    assert!(
        !target.display.contains("s3cret"),
        "the report names no credential: {}",
        target.display
    );
    assert_eq!(
        target
            .kms_path
            .as_deref()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned()),
        Some("clio.kms.json".to_owned())
    );
}

/// A backend this build does not snapshot is refused, and the message names the
/// two it does accept.
#[test]
fn an_unsupported_backend_is_refused_with_the_two_it_accepts() {
    let parsed = store_flags(&["--db", "./memory.db", "--backend", "duckdb"]);

    let error = resolve_target(&no_env(), &parsed, "create")
        .map(drop)
        .expect_err("a snapshot cannot read a backend this build does not have");

    assert_eq!(error.code(), "usage");
    assert_eq!(
        error.message(),
        "`clio backup create` does not support the duckdb backend"
    );
    assert_eq!(
        error.hint(),
        Some("backend must be sqlite or postgres, got `duckdb`")
    );
}

/// `--latest` restores the newest row of the listing, which is the first row.
#[test]
fn the_latest_flag_selects_the_first_listed_row() {
    let rows = [row("newest"), row("older")];

    let id = resolve_restore_id(&restore_flags(&["--latest"]), &rows).expect("the newest row");

    assert_eq!(id, "newest");
}

/// `--latest` on an empty backup directory is refused, and the hint names the
/// command that would fill it.
#[test]
fn the_latest_flag_refuses_an_empty_backup_directory() {
    let error = resolve_restore_id(&restore_flags(&["--latest"]), &[])
        .expect_err("there is nothing to restore");

    assert_eq!(error.code(), "not_found");
    assert!(
        error.message().contains("there is nothing to restore"),
        "{error:?}"
    );
    assert!(
        error
            .hint()
            .is_some_and(|hint| hint.contains("clio backup create")),
        "the hint names the command that makes a snapshot: {error:?}"
    );
}

/// A positional id the listing holds is restored as typed, even when it is not
/// the newest row.
#[test]
fn a_positional_id_that_is_listed_is_selected() {
    let rows = [row("newest"), row("older")];

    let id = resolve_restore_id(&restore_flags(&["older"]), &rows).expect("a listed id");

    assert_eq!(id, "older");
}

/// A mistyped id is reported with the closest listed id, so a restored-from
/// typo names the row the operator probably meant.
#[test]
fn a_mistyped_id_is_reported_with_the_closest_listed_one() {
    let rows = [row("nightly-20261101")];

    let error = resolve_restore_id(&restore_flags(&["nightly-20261102"]), &rows)
        .expect_err("no row carries the typed id");

    assert_eq!(error.code(), "not_found");
    assert!(
        error.message().contains("no snapshot `nightly-20261102`"),
        "{error:?}"
    );
    assert_eq!(error.hint(), Some("did you mean `nightly-20261101`?"));
}

/// An id with no close neighbour gets no suggestion, and the hint names the
/// command that prints the real ids instead.
#[test]
fn an_id_with_no_close_neighbour_names_the_list_command() {
    let rows = [row("nightly")];

    let error = resolve_restore_id(&restore_flags(&["weekly"]), &rows)
        .expect_err("no row carries the typed id");

    assert_eq!(error.code(), "not_found");
    assert_eq!(
        error.hint(),
        Some("run `clio backup list` to see the snapshots")
    );
}
