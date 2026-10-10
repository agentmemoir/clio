// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that the effective config's store defaults agree with the
//! shared database resolver the CLI uses, and that a value written
//! into the config file actually decides which store a command opens.
//!
//! ## Owns
//! - Config-vs-resolver default agreement over an injected environment
//! - Explicit config value precedence over the derived default
//! - The address/backend pairing, in both directions, through a real config file
//!
//! ## Does not own
//! - The resolver itself (see `crate::db_path` and `crate::db_effective`)
//! - CLI flag handling (`clio`)
//!
//! ## Boundary
//!
//! Tests here break when the config store defaults or resolver change; they
//! never change the implementation.
//!
//! Keep this module focused on config/resolver agreement.

use crate::config::*;
use crate::db_effective::resolve_effective_database;
use crate::error::ToolResult;
use crate::secret_seal::test_key;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The key every sealed value in this file is written under, so a test never
/// reaches the machine's key store.
const KEY: [u8; crate::secret_key::KEY_LEN] = [0x3A; crate::secret_key::KEY_LEN];

fn home_env(home: &str) -> impl Fn(&str) -> Option<String> {
    let home = home.to_owned();
    move |key: &str| (key == "HOME").then(|| home.clone())
}

/// A unique path for one test's config file, under the system temp directory.
fn temp_config(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("am-db-agree-{tag}-{nanos}.json"))
}

/// Boot a runtime whose only config is the given file body, then resolve the
/// target the way a command does. The file is removed on the way out.
fn resolve_through_file(tag: &str, body: &str) -> Result<crate::db_path::ResolvedDatabase, String> {
    let path = temp_config(tag);
    std::fs::write(&path, body).expect("write config");
    let resolved = resolve_through_file_at(&path);
    let _ = std::fs::remove_file(&path);
    resolved
}

/// The same resolution, against a config file the caller removes itself.
fn resolve_through_file_at(path: &Path) -> Result<crate::db_path::ResolvedDatabase, String> {
    let get = |key: &str| match key {
        "HOME" => Some("/tmp/am-db-agree-home".to_owned()),
        "CLIO_DEPLOYMENT_CONFIG" => Some(path.to_string_lossy().into_owned()),
        _ => None,
    };
    let runtime = Runtime::with_env(&get);
    resolve_effective_database(&get, &runtime.effective_raw(), None, None)
}

/// The same resolution, against a config written through the save path.
///
/// A value at a secret path must be sealed, or the loader refuses the file and
/// the configured address never reaches the resolver. Saving through
/// [`save_overlay_file`] is what `clio setup` does, so the loader sees the
/// same file a real deployment would.
fn resolve_through_saved(
    tag: &str,
    key: &str,
    url: &str,
) -> Result<crate::db_path::ResolvedDatabase, String> {
    test_key::use_key(KEY);
    let file = temp_config(tag);
    let mut overlay = Overlay::new();
    overlay.insert(key.to_owned(), json!(url));
    save_overlay_file(&file, &overlay).map_err(|e| e.message)?;
    let resolved = resolve_through_file_at(&file);
    let _ = std::fs::remove_file(&file);
    resolved
}

#[test]
fn config_store_defaults_match_the_resolver() {
    let get = home_env("/tmp/am-t31-10-home");
    let rt = Runtime::with_env(&get);

    let expected_path = crate::db_path::default_sqlite_path(&get);
    let ToolResult::Ok { data, .. } = rt.config_get(Some("store.sqlite_path")) else {
        panic!("config_get store.sqlite_path");
    };
    assert_eq!(data.value, json!(expected_path.to_string_lossy()));

    let resolved = crate::db_path::resolve_database(&get, None, None);
    assert_eq!(resolved.url, expected_path.to_string_lossy());
    assert_eq!(
        rt.effective_raw()["store"]["database_url"],
        json!(resolved.url)
    );
}

#[test]
fn explicit_config_value_wins_over_the_derived_default() {
    let get = home_env("/tmp/am-t31-10-home-2");
    let mut rt = Runtime::with_env(&get);
    let ToolResult::Ok { .. } = rt.config_set(
        "store.database_url",
        json!("postgres://configured/db"),
        ConfigScope::Session,
    ) else {
        panic!("config_set store.database_url");
    };
    assert_eq!(
        rt.effective_raw()["store"]["database_url"],
        json!("postgres://configured/db")
    );
}

#[test]
fn database_url_env_is_reflected_in_the_config_default() {
    let get = |key: &str| match key {
        "CLIO_DATABASE_URL" => Some("postgres://env/db".to_owned()),
        "HOME" => Some("/tmp/clio-t31-10-home-3".to_owned()),
        _ => None,
    };
    let rt = Runtime::with_env(&get);
    assert_eq!(
        rt.effective_raw()["store"]["database_url"],
        json!("postgres://env/db")
    );
    assert_eq!(
        rt.effective_raw()["store"]["sqlite_path"],
        json!("/tmp/clio-t31-10-home-3/.local/share/clio/clio.db")
    );
    assert_eq!(
        rt.effective_raw()["backend"],
        json!(crate::db_path::infer_backend("postgres://env/db"))
    );
}

#[test]
fn explicit_backend_env_still_wins_over_scheme_inference() {
    let get = |key: &str| match key {
        "CLIO_DATABASE_URL" => Some("postgres://env/db".to_owned()),
        "CLIO_BACKEND" => Some("sqlite".to_owned()),
        _ => None,
    };
    let rt = Runtime::with_env(&get);
    assert_eq!(rt.effective_raw()["backend"], json!("sqlite"));
    assert_eq!(
        rt.effective_raw()["store"]["database_url"],
        json!("postgres://env/db")
    );
}

/// The silent-SQLite regression, at the real entry point: a deployment config
/// that names a Postgres address must decide the target, whatever the seeded
/// `backend` default says.
#[test]
fn a_configured_address_decides_the_target() {
    let resolved = resolve_through_saved(
        "address-only",
        "store.database_url",
        "postgres://configured/db",
    )
    .expect("a configured address is openable");
    assert_eq!(resolved.backend, "postgres");
    assert_eq!(resolved.url, "postgres://configured/db");
}

/// The mirror of the pairing: a config that names only the backend resolves to
/// that backend, so the store's own open refuses it loudly instead of the
/// command quietly opening a local file.
#[test]
fn a_configured_backend_without_an_address_stays_that_backend() {
    let resolved = resolve_through_file("backend-only", r#"{"backend":"postgres"}"#)
        .expect("a named backend is resolvable");
    assert_eq!(resolved.backend, "postgres");
}

/// The deployment overlay is merged above the process environment, so the
/// config file `clio setup` writes outranks a `CLIO_DATABASE_URL` exported in
/// the operator's shell. The wizard has to have the last word for the bug this
/// resolver fixes to stay fixed.
#[test]
fn a_deployment_config_outranks_the_database_url_environment() {
    let path = temp_config("deploy-over-env");
    // The file must reach the loader the way `clio setup` leaves it: the
    // address sits at a secret path, so the save path seals it and the
    // loader's gate accepts the file.
    test_key::use_key(KEY);
    let mut overlay = Overlay::new();
    overlay.insert(
        "store.database_url".to_owned(),
        json!("postgres://configured/db"),
    );
    save_overlay_file(&path, &overlay).expect("save a sealed config");
    // The environment lookup is the one a command uses, with the config file
    // named by the deployment-path override and `CLIO_DATABASE_URL` folded in
    // below the deployment overlay by the runtime itself.
    let get = |key: &str| match key {
        "CLIO_DATABASE_URL" => Some("postgres://env/db".to_owned()),
        "CLIO_DEPLOYMENT_CONFIG" => Some(path.to_string_lossy().into_owned()),
        _ => None,
    };
    let runtime = Runtime::with_env(&get);
    let resolved =
        resolve_effective_database(&get, &runtime.effective_raw(), None, None).expect("resolves");
    let _ = std::fs::remove_file(&path);
    assert_eq!(resolved.url, "postgres://configured/db");
    assert_eq!(resolved.backend, "postgres");
}

/// The process environment sits above the config file, so an operator who
/// exports `CLIO_DATABASE_URL` and has no deployment config still gets it.
#[test]
fn the_database_url_environment_overrides_the_config_file() {
    let get = |key: &str| match key {
        "CLIO_DATABASE_URL" => Some("postgres://env/db".to_owned()),
        _ => None,
    };
    let mut runtime = Runtime::with_env(&get);
    runtime
        .load_file_json(r#"{"store":{"database_url":"postgres://from-file/db"}}"#)
        .expect("load the config file");
    let resolved =
        resolve_effective_database(&get, &runtime.effective_raw(), None, None).expect("resolves");
    assert_eq!(resolved.url, "postgres://env/db");
    assert_eq!(resolved.backend, "postgres");
}

/// A malformed address in the config file is reported, not turned into a local
/// SQLite file.
#[test]
fn a_malformed_configured_address_is_refused() {
    let error = resolve_through_saved("malformed", "store.database_url", "mysql://configured/db")
        .expect_err("a mysql server cannot be opened");
    assert!(error.contains("mysql://configured/db"), "{error}");
}
