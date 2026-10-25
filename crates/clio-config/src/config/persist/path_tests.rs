// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for behavior tests of deployment-path resolution:
//! the explicit override, the `CLIO_DEPLOYMENT_CONFIG` override and its disable
//! rule, the XDG default directory, and the resolved-path accessor on `Runtime`.
//!
//! ## Owns
//! - Behavior tests for `config::persist::path`
//!
//! ## Does not own
//! - The path resolution under test
//! - Reading or writing the overlay file (see the `persist` test modules)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::config::Runtime;

/// Lookup closure over a fixed table, so no test here reads the host environment.
fn lookup<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |key: &str| {
        pairs
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).to_owned())
    }
}

#[test]
fn explicit_override_wins_over_every_environment_tier() {
    let explicit = PathBuf::from("/tmp/explicit/deployment.json");
    let env = lookup(&[
        (DEPLOYMENT_CONFIG_ENV, "/tmp/from-env.json"),
        ("XDG_CONFIG_HOME", "/tmp/xdg"),
    ]);
    assert_eq!(
        resolve_deployment_path_with(Some(explicit.clone()), &env),
        Some(explicit.clone()),
        "an explicit path is used as given"
    );
    assert_eq!(
        resolve_deployment_path(Some(explicit.clone())),
        Some(explicit),
        "the public entry point forwards the explicit override"
    );
}

#[test]
fn env_override_is_trimmed_and_a_blank_value_disables_loading() {
    // A padded value names the same file once trimmed.
    let padded = lookup(&[(DEPLOYMENT_CONFIG_ENV, "  /tmp/padded.json  ")]);
    assert_eq!(
        resolve_deployment_path_with(None, &padded),
        Some(PathBuf::from("/tmp/padded.json")),
        "the override is trimmed before it becomes a path"
    );
    // Blank is "disabled", not "a file in the current directory", so the XDG
    // default is not used either.
    let blank = lookup(&[
        (DEPLOYMENT_CONFIG_ENV, "   "),
        ("XDG_CONFIG_HOME", "/tmp/xdg"),
    ]);
    assert_eq!(resolve_deployment_path_with(None, &blank), None);
}

#[test]
fn xdg_supplies_the_default_path_when_no_override_is_set() {
    let xdg = lookup(&[("XDG_CONFIG_HOME", "/tmp/xdg"), ("HOME", "/tmp/home")]);
    assert_eq!(
        resolve_deployment_path_with(None, &xdg),
        Some(PathBuf::from("/tmp/xdg/clio/deployment.json")),
        "the default is the app config directory plus the deployment file name"
    );
}

#[test]
fn default_config_dir_requires_an_absolute_base() {
    let both = lookup(&[("XDG_CONFIG_HOME", "/tmp/xdg"), ("HOME", "/tmp/home")]);
    assert_eq!(
        default_config_dir(&both),
        Some(PathBuf::from("/tmp/xdg/clio")),
        "an absolute XDG base wins over HOME"
    );
    // XDG requires absolute paths, so a relative base is ignored and HOME is next.
    let relative_xdg = lookup(&[("XDG_CONFIG_HOME", "rel"), ("HOME", "/tmp/home")]);
    assert_eq!(
        default_config_dir(&relative_xdg),
        Some(PathBuf::from("/tmp/home/.config/clio"))
    );
    // A blank HOME names no directory at all.
    let blank_home = lookup(&[("HOME", "   ")]);
    assert_eq!(default_config_dir(&blank_home), None);
    // Neither does a relative one.
    let relative_home = lookup(&[("HOME", "rel")]);
    assert_eq!(default_config_dir(&relative_home), None);
    // And with no value at all there is nothing to resolve.
    assert_eq!(default_config_dir(&|_key: &str| None), None);
}

#[test]
fn runtime_reports_whether_a_deployment_path_resolved() {
    // No host environment, so no deployment path resolves and a deployment-scoped
    // set has nowhere durable to go.
    let mut rt = Runtime::with_env(&|_key: &str| None);
    assert!(!rt.has_deployment_path());
    // A named path makes the deployment scope durable again.
    rt.set_deployment_path("/tmp/explicit/deployment.json");
    assert!(rt.has_deployment_path());
    // Dropping it keeps a later deployment-scoped set in memory only.
    rt.clear_deployment_path();
    assert!(!rt.has_deployment_path());
}
