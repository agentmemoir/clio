// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit-testing the backup-directory resolver
//! and the store-slug helper.
//!
//! ## Owns
//! - The precedence matrix (env tier, data-directory tier, relative fallback)
//! - The rejection cases (empty, whitespace, NUL) and the always-absolute rule
//! - Slug safety: SQLite stems, Postgres URL names, credential exclusion,
//!   degenerate-target fallback, and the length bound
//!
//! ## Does not own
//! - The resolver implementation (see `backup_path`)
//! - Database-target resolution tests (see `db_path_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the backup precedence or slug rules change; they
//! never change the implementation.
//!
//! Keep this module focused on backup-path unit behavior.

use super::*;
use std::path::Path;

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

const CWD: &str = "/working/dir";

#[test]
fn env_override_wins_over_every_other_tier() {
    let get = env(&[
        (BACKUP_DIR_ENV, "/explicit/backups"),
        ("CLIO_DATA_DIR", "/data"),
        ("XDG_DATA_HOME", "/xdg"),
        ("HOME", "/home"),
    ]);
    let resolved = resolve_backup_dir(&get, Path::new(CWD));
    assert_eq!(resolved.path, Path::new("/explicit/backups"));
    assert_eq!(resolved.tier, BackupDirTier::EnvOverride);
}

#[test]
fn env_override_rejects_empty_whitespace_and_nul() {
    for value in ["", "   ", "with\0nul"] {
        let get = env(&[(BACKUP_DIR_ENV, value), ("CLIO_DATA_DIR", "/data")]);
        let resolved = resolve_backup_dir(&get, Path::new(CWD));
        assert_eq!(resolved.tier, BackupDirTier::DataDirDefault, "{value:?}");
    }
}

#[test]
fn env_override_relative_value_is_resolved_against_current_dir() {
    let get = env(&[(BACKUP_DIR_ENV, "rel/backups")]);
    let resolved = resolve_backup_dir(&get, Path::new(CWD));
    assert_eq!(resolved.path, Path::new("/working/dir/rel/backups"));
    assert_eq!(resolved.tier, BackupDirTier::EnvOverride);
}

#[test]
fn data_dir_default_is_the_sqlite_parent_plus_backups() {
    let get = env(&[("CLIO_DATA_DIR", "/data")]);
    let resolved = resolve_backup_dir(&get, Path::new(CWD));
    assert_eq!(resolved.path, Path::new("/data/backups"));
    assert_eq!(resolved.tier, BackupDirTier::DataDirDefault);
}

#[test]
fn data_dir_default_follows_xdg_then_home_tiers() {
    let xdg = env(&[("XDG_DATA_HOME", "/xdg")]);
    assert_eq!(
        resolve_backup_dir(&xdg, Path::new(CWD)).path,
        Path::new("/xdg/clio/backups")
    );
    let home = env(&[("HOME", "/home/u")]);
    assert_eq!(
        resolve_backup_dir(&home, Path::new(CWD)).path,
        Path::new("/home/u/.local/share/clio/backups")
    );
}

#[test]
fn relative_fallback_is_absolute_against_current_dir() {
    let resolved = resolve_backup_dir(&none, Path::new(CWD));
    assert_eq!(resolved.path, Path::new("/working/dir/clio-backups"));
    assert_eq!(resolved.tier, BackupDirTier::RelativeFallback);
    assert!(resolved.path.is_absolute());
}

#[test]
fn default_backup_dir_agrees_with_the_resolver_default_tier() {
    assert_eq!(
        default_backup_dir(&none, Path::new(CWD)).to_string_lossy(),
        "/working/dir/clio-backups"
    );
    let get = env(&[("CLIO_DATA_DIR", "/data")]);
    assert_eq!(
        default_backup_dir(&get, Path::new(CWD)).to_string_lossy(),
        "/data/backups"
    );
}

#[test]
fn relative_data_dir_value_still_resolves_absolute() {
    // A relative CLIO_DATA_DIR makes default_sqlite_path relative; the backup
    // directory must still come back absolute.
    let get = env(&[("CLIO_DATA_DIR", "rel-data")]);
    let resolved = resolve_backup_dir(&get, Path::new(CWD));
    assert_eq!(resolved.path, Path::new("/working/dir/rel-data/backups"));
    assert!(resolved.path.is_absolute());
}

#[test]
fn tier_names_are_stable() {
    assert_eq!(
        BackupDirTier::EnvOverride.as_str(),
        "CLIO_BACKUP_DIR override"
    );
    assert_eq!(
        BackupDirTier::DataDirDefault.as_str(),
        "data directory default"
    );
    assert_eq!(
        BackupDirTier::RelativeFallback.as_str(),
        "current-directory fallback"
    );
}

#[test]
fn slug_from_sqlite_path_is_the_file_stem() {
    assert_eq!(
        store_slug("/home/u/.local/share/clio/clio.db", "sqlite"),
        "clio"
    );
    assert_eq!(store_slug("/var/data/store.db", "sqlite"), "store");
    assert_eq!(
        store_slug("sqlite:///var/data/store.sqlite3", "sqlite"),
        "store"
    );
    assert_eq!(store_slug("sqlite:store.db", "sqlite"), "store");
}

#[test]
fn slug_from_postgres_url_is_the_path_database_name() {
    let slug = store_slug("postgres://user:pw@host:5432/prod?x=1", "postgres");
    assert_eq!(slug, "prod");
    assert!(!slug.contains("pw"));
    assert!(!slug.contains(':'));
    assert!(!slug.contains('/'));
    assert_eq!(store_slug("postgresql://u:p@h/appdb", "postgres"), "appdb");
}

#[test]
fn slug_never_carries_a_credential_fragment() {
    // The password is engineered to be the entire remaining URL after a naive
    // split; only the path segment may survive.
    let slug = store_slug("postgres://pw:pw@pw.example/pwdb", "postgres");
    assert_eq!(slug, "pwdb");
    // No whitespace, separator, or scheme ever survives.
    for url in ["postgres://a b:c@h/db name", "postgres://u:p@h/db\ttab"] {
        let slug = store_slug(url, "postgres");
        assert!(
            !slug.chars().any(char::is_whitespace),
            "{url:?} -> {slug:?}"
        );
        assert!(!slug.contains('/'), "{url:?} -> {slug:?}");
        assert!(!slug.contains(':'), "{url:?} -> {slug:?}");
    }
}

#[test]
fn slug_replaces_unsafe_characters_instead_of_refusing() {
    assert_eq!(store_slug("/data/my store.db", "sqlite"), "my_store");
    assert_eq!(store_slug("/data/ünicode.db", "sqlite"), "_nicode");
}

#[test]
fn slug_is_bounded_in_length() {
    let long = format!("/data/{}.db", "x".repeat(200));
    let slug = store_slug(&long, "sqlite");
    assert_eq!(slug.len(), MAX_SLUG_BYTES);
    assert!(
        slug.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    );
}

#[test]
fn degenerate_slug_falls_back_to_a_short_hash() {
    let slug = store_slug("", "sqlite");
    assert!(slug.starts_with("db-"));
    let hex = slug.trim_start_matches("db-");
    assert_eq!(hex.len(), 8);
    assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    // Deterministic for the same target, distinct across targets.
    assert_eq!(slug, store_slug("", "sqlite"));
    assert_ne!(slug, store_slug("postgres://h/other", "postgres"));
}

#[test]
fn empty_postgres_path_falls_back_to_a_hash() {
    let slug = store_slug("postgres://u:p@host/", "postgres");
    assert!(slug.starts_with("db-"));
    assert!(!slug.contains("p@host"));
}

#[test]
fn empty_and_parentdir_targets_fall_back_to_a_hash() {
    // An empty URL and a parent-directory component have no file stem.
    assert!(store_slug("", "sqlite").starts_with("db-"));
    assert!(store_slug("/data/..", "sqlite").starts_with("db-"));
}

#[test]
fn in_memory_target_slug_is_safe() {
    let slug = store_slug("sqlite::memory:", "sqlite");
    assert_eq!(slug, "_memory_");
    assert!(!slug.chars().any(char::is_whitespace));
    assert!(!slug.contains(':'));
}

/// Assert that no piece of a URL's userinfo or host survives into a slug, and
/// that the slug carries no separator.
fn assert_slug_leaks_nothing(url: &str, slug: &str) {
    for forbidden in [
        "user",
        "pw",
        "hunter2",
        "s3cret",
        "db.example.com",
        "host",
        "@",
        "/",
    ] {
        assert!(
            !slug.contains(forbidden),
            "{url:?} -> {slug:?} ({forbidden:?})"
        );
    }
}

#[test]
fn slug_from_url_without_a_path_never_carries_userinfo() {
    // A no-path URL is normal: the driver defaults the database to the user
    // name. There is no path segment to read, so the slug must be the hash.
    for url in [
        "postgres://user:pw@host:5432",
        "postgres://user:pw@host",
        "postgresql://clio:hunter2@db.example.com:5432",
        "postgres://host:5432",
    ] {
        let slug = store_slug(url, "postgres");
        assert_slug_leaks_nothing(url, &slug);
        assert!(slug.starts_with("db-"), "{url:?} -> {slug:?}");
        let hex = slug.trim_start_matches("db-");
        assert_eq!(hex.len(), 8, "{url:?} -> {slug:?}");
        assert!(
            hex.chars().all(|c| c.is_ascii_hexdigit()),
            "{url:?} -> {slug:?}"
        );
    }
}

#[test]
fn slug_uses_the_path_not_a_slash_inside_the_userinfo() {
    // A raw `/` in a password is not valid URL syntax, so the authority
    // boundary is ambiguous. The name is dropped and the hash fallback runs;
    // userinfo is never emitted whatever the split would have said.
    for (url, forbidden) in [
        // A raw `/` in the password: the authority cannot be located safely,
        // so nothing is guessed and the hash fallback carries the slug.
        ("postgres://user:pa/ss@host/db", "ss"),
        ("postgres://user:pa/ss@host", "ss"),
        // A `/` in the user name is the same ambiguity.
        ("postgres://us/er:pw@host/prod", "er"),
    ] {
        let slug = store_slug(url, "postgres");
        assert!(!slug.contains(forbidden), "{url:?} -> {slug:?}");
        assert!(!slug.contains("user"), "{url:?} -> {slug:?}");
        assert!(!slug.contains("pw"), "{url:?} -> {slug:?}");
        assert!(!slug.contains('@'), "{url:?} -> {slug:?}");
        assert!(slug.starts_with("db-"), "{url:?} -> {slug:?}");
    }
    // The encoded form of the same password has no slash: the name resolves.
    let encoded = store_slug("postgres://user:pa%2Fss@host/db", "postgres");
    assert_eq!(encoded, "db");
    assert!(!encoded.contains("user"), "{encoded:?}");
    assert!(!encoded.contains("ss@host"), "{encoded:?}");
}

#[test]
fn slug_uses_the_path_when_a_query_or_fragment_carries_a_slash() {
    // The query and fragment go first, so their `/` never becomes a path.
    let cases = [
        ("postgres://host/db?redirect=/other", "db"),
        ("postgres://host/db#frag/other", "db"),
        ("postgres://u:p@host:5432?x=/admin/secret", ""),
        ("postgres://u:p@host:5432#x=/admin/secret", ""),
    ];
    for (url, name) in cases {
        let slug = store_slug(url, "postgres");
        assert_slug_leaks_nothing(url, &slug);
        if name.is_empty() {
            assert!(slug.starts_with("db-"), "{url:?} -> {slug:?}");
        } else {
            assert_eq!(slug, name, "{url:?} -> {slug:?}");
        }
    }
}

#[test]
fn url_without_a_scheme_never_reaches_the_slug_as_userinfo() {
    // Bare targets are not URL authority + path. Either the name is read
    // from the last segment or the whole value falls back to the hash; the
    // userinfo is never taken as a database name.
    for url in ["u:p@host/db", "u:p@host", "u:p@host:5432"] {
        let slug = store_slug(url, "postgres");
        assert!(!slug.contains("u_p_host"), "{url:?} -> {slug:?}");
        if let Some(name) = slug.strip_prefix("db-") {
            assert_eq!(name.len(), 8, "{url:?} -> {slug:?}");
        } else {
            assert_eq!(slug, "db", "{url:?} -> {slug:?}");
        }
    }
}
