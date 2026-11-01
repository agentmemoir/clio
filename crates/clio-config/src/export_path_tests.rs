// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit-testing the export-root resolver.
//!
//! ## Owns
//! - The precedence matrix (env tier, data-directory tier, relative fallback)
//! - The rejection cases (empty, whitespace, NUL) and the always-absolute rule
//! - The stability of the tier names and the agreement of the environment-only
//!   view with the resolver
//!
//! ## Does not own
//! - The resolver implementation (see `export_path`)
//! - Backup-directory resolution tests (see `backup_path_tests`)
//! - Destination confinement inside the root (see the delivery seam)
//!
//! ## Boundary
//!
//! Tests here break when the export-root precedence rules change; they never
//! change the implementation.
//!
//! Keep this module focused on export-path unit behavior.

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
        (EXPORT_ROOT_ENV, "/explicit/exports"),
        ("CLIO_DATA_DIR", "/data"),
        ("XDG_DATA_HOME", "/xdg"),
        ("HOME", "/home"),
    ]);
    let resolved = resolve_export_root(&get, Path::new(CWD));
    assert_eq!(resolved.path, Path::new("/explicit/exports"));
    assert_eq!(resolved.tier, ExportRootTier::EnvOverride);
}

#[test]
fn env_override_rejects_empty_whitespace_and_nul() {
    for value in ["", "   ", "with\0nul"] {
        let get = env(&[(EXPORT_ROOT_ENV, value), ("CLIO_DATA_DIR", "/data")]);
        let resolved = resolve_export_root(&get, Path::new(CWD));
        assert_eq!(resolved.path, Path::new("/data/exports"), "{value:?}");
        assert_eq!(resolved.tier, ExportRootTier::DataDirDefault, "{value:?}");
    }
}

#[test]
fn env_override_relative_value_is_resolved_against_current_dir() {
    let get = env(&[(EXPORT_ROOT_ENV, "rel/exports")]);
    let resolved = resolve_export_root(&get, Path::new(CWD));
    assert_eq!(resolved.path, Path::new("/working/dir/rel/exports"));
    assert_eq!(resolved.tier, ExportRootTier::EnvOverride);
    assert!(resolved.path.is_absolute());
}

#[test]
fn data_dir_default_is_the_sqlite_parent_plus_exports() {
    let get = env(&[("CLIO_DATA_DIR", "/data")]);
    let resolved = resolve_export_root(&get, Path::new(CWD));
    assert_eq!(resolved.path, Path::new("/data/exports"));
    assert_eq!(resolved.tier, ExportRootTier::DataDirDefault);
}

#[test]
fn data_dir_default_follows_xdg_then_home_tiers() {
    let xdg = env(&[("XDG_DATA_HOME", "/xdg")]);
    assert_eq!(
        resolve_export_root(&xdg, Path::new(CWD)).path,
        Path::new("/xdg/clio/exports")
    );
    let home = env(&[("HOME", "/home/u")]);
    assert_eq!(
        resolve_export_root(&home, Path::new(CWD)).path,
        Path::new("/home/u/.local/share/clio/exports")
    );
}

#[test]
fn relative_fallback_is_absolute_against_current_dir() {
    let resolved = resolve_export_root(&none, Path::new(CWD));
    assert_eq!(resolved.path, Path::new("/working/dir/clio-exports"));
    assert_eq!(resolved.tier, ExportRootTier::RelativeFallback);
    assert!(resolved.path.is_absolute());
}

#[test]
fn relative_data_dir_value_still_resolves_absolute() {
    // A relative CLIO_DATA_DIR makes the default SQLite path relative; the
    // export root must still come back absolute.
    let get = env(&[("CLIO_DATA_DIR", "rel-data")]);
    let resolved = resolve_export_root(&get, Path::new(CWD));
    assert_eq!(resolved.path, Path::new("/working/dir/rel-data/exports"));
    assert!(resolved.path.is_absolute());
}

#[test]
fn tier_names_are_stable() {
    assert_eq!(ExportRootTier::EnvOverride.as_str(), "environment override");
    assert_eq!(
        ExportRootTier::DataDirDefault.as_str(),
        "data directory default"
    );
    assert_eq!(
        ExportRootTier::RelativeFallback.as_str(),
        "relative fallback"
    );
}

#[test]
fn default_export_root_agrees_with_the_resolver() {
    let explicit = env(&[(EXPORT_ROOT_ENV, "/explicit/exports")]);
    assert_eq!(
        default_export_root(&explicit),
        resolve_export_root(&explicit, Path::new(CWD)).path
    );
    let data_dir = env(&[("CLIO_DATA_DIR", "/data")]);
    assert_eq!(
        default_export_root(&data_dir),
        resolve_export_root(&data_dir, Path::new(CWD)).path
    );
}

#[test]
fn default_export_root_names_the_fallback_component_without_a_current_dir() {
    // The environment-only view has no current directory to resolve against,
    // so the last-resort tier is the bare component the resolver joins.
    let fallback = default_export_root(&none);
    assert_eq!(fallback, Path::new(FALLBACK_DIR_NAME));
    assert_eq!(
        resolve_export_root(&none, Path::new(CWD)).path,
        Path::new(CWD).join(&fallback)
    );
}
