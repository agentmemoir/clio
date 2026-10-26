// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of deployment-path and config-directory
//! resolution from an explicit value or the injected environment.
//!
//! ## Owns
//! - Resolution behavior under test for `config::persist::path`
//!
//! ## Does not own
//! - Production path resolution under test
//! - Reading, writing, or backing up the overlay file
//!
//! ## Boundary
//!
//! Changes to how a deployment path or config directory resolves should normally
//! update tests here. Changes to overlay file contents should not.
//!
//! Keep this module focused on deployment and config path resolution tests.

use super::*;

fn lookup<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |key: &str| {
        pairs
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).to_owned())
    }
}

#[test]
fn has_deployment_path_reports_whether_one_resolved() {
    // No explicit path and no environment: nothing to persist to.
    let mut rt = Runtime::with_env(&|_| None);
    assert!(!rt.has_deployment_path());
    // An explicit deployment file is a resolved path.
    let explicit = Runtime::with_deployment_file("/tmp/has-path/deployment.json");
    assert!(explicit.has_deployment_path());
    // Dropping the path again withdraws the promise.
    rt.set_deployment_path("/tmp/has-path/deployment.json");
    assert!(rt.has_deployment_path());
    rt.clear_deployment_path();
    assert!(!rt.has_deployment_path());
}

#[test]
fn resolve_prefers_explicit() {
    let p = PathBuf::from("/tmp/explicit.json");
    assert_eq!(resolve_deployment_path(Some(p.clone())), Some(p));
}

#[test]
fn resolve_env_override_and_disable() {
    // Env override wins over the XDG default.
    let env = lookup(&[
        ("CLIO_DEPLOYMENT_CONFIG", "/tmp/e.json"),
        ("XDG_CONFIG_HOME", "/tmp/xdg"),
    ]);
    assert_eq!(
        resolve_deployment_path_with(None, &env),
        Some(PathBuf::from("/tmp/e.json"))
    );
    // An empty override disables loading entirely.
    let disabled = lookup(&[
        ("CLIO_DEPLOYMENT_CONFIG", ""),
        ("XDG_CONFIG_HOME", "/tmp/xdg"),
    ]);
    assert_eq!(resolve_deployment_path_with(None, &disabled), None);
}

#[test]
fn resolve_defaults_follow_xdg_then_home() {
    let xdg = lookup(&[("XDG_CONFIG_HOME", "/tmp/xdg"), ("HOME", "/tmp/home")]);
    assert_eq!(
        resolve_deployment_path_with(None, &xdg),
        Some(PathBuf::from("/tmp/xdg/clio/deployment.json"))
    );
    let home = lookup(&[("HOME", "/tmp/home")]);
    assert_eq!(
        resolve_deployment_path_with(None, &home),
        Some(PathBuf::from("/tmp/home/.config/clio/deployment.json"))
    );
    // A relative XDG base is invalid and ignored; HOME is the fallback.
    let relative = lookup(&[("XDG_CONFIG_HOME", "rel"), ("HOME", "/tmp/home")]);
    assert_eq!(
        resolve_deployment_path_with(None, &relative),
        Some(PathBuf::from("/tmp/home/.config/clio/deployment.json"))
    );
    // No home at all: nothing to load.
    assert_eq!(resolve_deployment_path_with(None, |_| None), None);
    // A relative HOME is also invalid: nothing usable to load.
    let relative_home = lookup(&[("HOME", "rel")]);
    assert_eq!(resolve_deployment_path_with(None, &relative_home), None);
}

#[test]
fn default_config_dir_is_the_app_dir_under_an_absolute_base() {
    let xdg = lookup(&[("XDG_CONFIG_HOME", "/tmp/xdg"), ("HOME", "/tmp/home")]);
    assert_eq!(default_config_dir(&xdg), Some(PathBuf::from("/tmp/xdg/clio")));
    let home = lookup(&[("HOME", "/tmp/home")]);
    assert_eq!(
        default_config_dir(&home),
        Some(PathBuf::from("/tmp/home/.config/clio"))
    );
    // A blank value counts as unset.
    let blank = lookup(&[("XDG_CONFIG_HOME", "   "), ("HOME", "/tmp/home")]);
    assert_eq!(
        default_config_dir(&blank),
        Some(PathBuf::from("/tmp/home/.config/clio"))
    );
    // Every base relative or blank: no shared config directory.
    let none = lookup(&[("XDG_CONFIG_HOME", "rel"), ("HOME", "  ")]);
    assert_eq!(default_config_dir(&none), None);
}
