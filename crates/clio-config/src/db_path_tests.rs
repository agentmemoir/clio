// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit-testing the database-target resolver:
//! precedence tiers, backend inference, and path shapes.
//!
//! ## Owns
//! - Each precedence tier and its first-match-wins ordering
//! - Scheme-based backend inference and explicit-backend override
//! - Path-shape cases (trailing separators, empty/whitespace/NUL values)
//!
//! ## Does not own
//! - The resolver implementation (see `db_path`)
//! - Config-vs-resolver default agreement (see `config::db_agreement_tests`)
//! - End-to-end CLI behavior (`clio` integration tests)
//!
//! ## Boundary
//!
//! Tests here break when the resolver precedence or inference changes; they
//! never change the implementation.
//!
//! Keep this module focused on resolver unit behavior.

use super::*;

/// Build an injected environment lookup from key/value pairs.
fn env(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> + 'static {
    let pairs: Vec<(&'static str, &'static str)> = pairs.to_vec();
    move |key: &str| {
        pairs
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).to_owned())
    }
}

fn none(_: &str) -> Option<String> {
    None
}

#[test]
fn explicit_db_wins_over_every_env_tier() {
    let get = env(&[
        ("CLIO_DATABASE_URL", "postgres://ignored/db"),
        ("CLIO_DATA_DIR", "/ignored"),
        ("XDG_DATA_HOME", "/ignored-xdg"),
        ("HOME", "/ignored-home"),
    ]);
    let resolved = resolve_database(&get, Some("./x.db"), None);
    assert_eq!(resolved.url, "./x.db");
    assert_eq!(resolved.backend, "sqlite");
}

#[test]
fn explicit_memory_value_passes_through_untouched() {
    let get = env(&[("CLIO_DATABASE_URL", "postgres://ignored/db")]);
    let resolved = resolve_database(&get, Some("sqlite::memory:"), None);
    assert_eq!(resolved.url, "sqlite::memory:");
    assert_eq!(resolved.backend, "sqlite");
}

#[test]
fn empty_explicit_db_falls_through_to_env() {
    let get = env(&[("CLIO_DATABASE_URL", "sqlite::memory:")]);
    let resolved = resolve_database(&get, Some("   "), None);
    assert_eq!(resolved.url, "sqlite::memory:");
}

#[test]
fn the_database_url_wins_and_infers_postgres() {
    let get = env(&[
        ("CLIO_DATABASE_URL", "postgres://u:p@host:5432/am"),
        ("CLIO_DATA_DIR", "/ignored"),
    ]);
    let resolved = resolve_database(&get, None, None);
    assert_eq!(resolved.url, "postgres://u:p@host:5432/am");
    assert_eq!(resolved.backend, "postgres");
}

#[test]
fn postgresql_scheme_also_infers_postgres() {
    assert_eq!(infer_backend("postgresql://h/db"), "postgres");
}

#[test]
fn the_database_url_variable_resolves_the_target() {
    let get = env(&[("CLIO_DATABASE_URL", "postgres://from-am/db")]);
    let resolved = resolve_database(&get, None, None);
    assert_eq!(resolved.url, "postgres://from-am/db");
    assert_eq!(resolved.backend, "postgres");
}

/// The bare name is not a second spelling of the same variable, so a shell that
/// exports only it falls through to the data directory instead of silently
/// opening the other database.
#[test]
fn the_bare_database_url_name_is_not_read() {
    let get = env(&[
        ("DATABASE_URL", "postgres://ignored/db"),
        ("CLIO_DATA_DIR", "/data"),
    ]);
    assert_eq!(
        resolve_database(&get, None, None).url,
        "/data/clio.db",
        "the bare name must not decide the target"
    );
}

#[test]
fn the_database_url_beats_am_data_dir() {
    let get = env(&[
        ("CLIO_DATABASE_URL", "postgres://primary/db"),
        ("CLIO_DATA_DIR", "/data"),
    ]);
    assert_eq!(
        resolve_database(&get, None, None).url,
        "postgres://primary/db"
    );
}

#[test]
fn am_data_dir_resolves_with_file_name() {
    let get = env(&[
        ("CLIO_DATA_DIR", "/tmp/clio-test-data"),
        ("HOME", "/home/u"),
    ]);
    let resolved = resolve_database(&get, None, None);
    assert_eq!(resolved.url, "/tmp/clio-test-data/clio.db");
    assert_eq!(resolved.backend, "sqlite");
}

#[test]
fn am_data_dir_trailing_separator_is_normalized() {
    let get = env(&[("CLIO_DATA_DIR", "/tmp/clio-test-data/")]);
    assert_eq!(
        resolve_database(&get, None, None).url,
        "/tmp/clio-test-data/clio.db"
    );
}

#[test]
fn xdg_data_home_resolves_under_app_subdir() {
    let get = env(&[("XDG_DATA_HOME", "/xdg/data"), ("HOME", "/home/u")]);
    assert_eq!(
        resolve_database(&get, None, None).url,
        "/xdg/data/clio/clio.db"
    );
}

#[test]
fn home_falls_back_to_local_share() {
    let get = env(&[("HOME", "/home/u")]);
    assert_eq!(
        resolve_database(&get, None, None).url,
        "/home/u/.local/share/clio/clio.db"
    );
}

#[test]
fn last_resort_is_relative_file_name() {
    let resolved = resolve_database(&none, None, None);
    assert_eq!(resolved.url, "clio.db");
    assert_eq!(resolved.backend, "sqlite");
}

#[test]
fn empty_and_whitespace_values_are_ignored() {
    let get = env(&[
        ("CLIO_DATABASE_URL", "   "),
        ("CLIO_DATA_DIR", "  "),
        ("XDG_DATA_HOME", ""),
        ("HOME", "/home/u"),
    ]);
    assert_eq!(
        resolve_database(&get, None, None).url,
        "/home/u/.local/share/clio/clio.db"
    );
}

#[test]
fn nul_bearing_value_is_ignored() {
    let get = env(&[("CLIO_DATA_DIR", "/bad\0dir"), ("HOME", "/home/u")]);
    assert_eq!(
        resolve_database(&get, None, None).url,
        "/home/u/.local/share/clio/clio.db"
    );
}

#[test]
fn explicit_backend_overrides_scheme_inference() {
    let get = env(&[("CLIO_DATABASE_URL", "postgres://h/db")]);
    let resolved = resolve_database(&get, None, Some("sqlite"));
    assert_eq!(resolved.backend, "sqlite");
    assert_eq!(resolved.url, "postgres://h/db");
}

#[test]
fn unknown_explicit_backend_passes_through() {
    let get = env(&[("CLIO_DATABASE_URL", "postgres://h/db")]);
    let resolved = resolve_database(&get, None, Some("mysql"));
    assert_eq!(resolved.backend, "mysql");
}

/// A `sqlite://` address names a file, so the resolved target is the file and
/// not the string that still spells the scheme. Before this, the store opener
/// received the whole string and created a directory tree named `sqlite:` in the
/// working directory, while the key-store path and the backup slug — which both
/// strip the scheme — pointed somewhere else entirely.
#[test]
fn a_sqlite_scheme_is_stripped_from_the_resolved_file() {
    for (address, file) in [
        ("sqlite:///data/clio.db", "/data/clio.db"),
        ("sqlite://data/clio.db", "data/clio.db"),
        ("sqlite:data/clio.db", "data/clio.db"),
        ("sqlite://./clio.db", "./clio.db"),
        ("/data/clio.db", "/data/clio.db"),
        ("clio.db", "clio.db"),
    ] {
        assert_eq!(
            sqlite_file_path(address),
            file,
            "the --db form of {address}"
        );

        let from_flag = resolve_database(&none, Some(address), None);
        assert_eq!(from_flag.url, file, "resolved from --db {address}");
        assert_eq!(from_flag.backend, "sqlite", "inferred from {address}");

        let from_env = resolve_database(&env(&[("CLIO_DATABASE_URL", address)]), None, None);
        assert_eq!(
            from_env.url, file,
            "resolved from CLIO_DATABASE_URL {address}"
        );
        assert_eq!(from_env.backend, "sqlite", "inferred from {address}");
    }
}

/// The in-memory sentinel is an address, not a file, and every open path
/// compares against that exact string to choose the in-memory constructor.
/// Stripping its scheme would leave `:memory:`, which no caller looks for, so
/// the whole store would try to open a file called `:memory:` on disk.
#[test]
fn the_in_memory_sentinel_survives_the_strip_unchanged() {
    assert_eq!(sqlite_file_path("sqlite::memory:"), "sqlite::memory:");
    let resolved = resolve_database(&none, Some("sqlite::memory:"), None);
    assert_eq!(resolved.url, "sqlite::memory:");
    assert_eq!(resolved.backend, "sqlite");
}

/// A Postgres address is never stripped: its scheme is the address, and the
/// opener parses it. Only SQLite's scheme is a spelling of a path.
#[test]
fn a_postgres_address_keeps_its_scheme() {
    for address in [
        "postgres://user:pw@host:5432/am",
        "postgresql://user@host/am",
    ] {
        assert_eq!(
            sqlite_file_path(address),
            address,
            "{address} is not a file"
        );
        let resolved = resolve_database(&env(&[("CLIO_DATABASE_URL", address)]), None, None);
        assert_eq!(resolved.url, address);
        assert_eq!(resolved.backend, "postgres");
    }
}

#[test]
fn default_sqlite_path_prefers_am_data_dir_then_xdg_then_home() {
    assert_eq!(
        default_sqlite_path(&env(&[
            ("CLIO_DATA_DIR", "/a"),
            ("XDG_DATA_HOME", "/x"),
            ("HOME", "/h")
        ])),
        PathBuf::from("/a/clio.db")
    );
    assert_eq!(
        default_sqlite_path(&env(&[("XDG_DATA_HOME", "/x"), ("HOME", "/h")])),
        PathBuf::from("/x/clio/clio.db")
    );
    assert_eq!(
        default_sqlite_path(&env(&[("HOME", "/h")])),
        PathBuf::from("/h/.local/share/clio/clio.db")
    );
    assert_eq!(default_sqlite_path(&none), PathBuf::from(DB_FILE_NAME));
}
