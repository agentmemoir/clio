// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio status` usage-error and help surfaces, plus one
//! end-to-end run over the real wiring (with the host key-store probe replaced)
//! against an in-memory SQLite store.
//!
//! ## Owns
//! - Exit-code 2 usage tests, `--help`, and the real-wiring integration test
//!
//! ## Does not own
//! - The injected-probe behavior tests (`status_cli_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the status exit-code contract or help surface changes;
//! they never change it.
//!
//! Keep this module focused on status usage and end-to-end failures.

use super::*;

struct Idle;

impl StatusDeps for Idle {
    fn env(&self, _key: &str) -> Option<String> {
        None
    }
    fn open_store(&self, _resolved: &ResolvedDatabase) -> Result<Box<dyn StatusRepos>, AmError> {
        unreachable!("usage paths never open the store")
    }
    fn embedder(&self) -> Result<Option<Arc<dyn Embedder>>, AmError> {
        unreachable!("usage paths never build an embedder")
    }
    fn scan_ports(&self) -> Vec<u16> {
        Vec::new()
    }
    fn compose(&self, _root: &Path) -> ComposeView {
        ComposeView::NotMaterialized
    }
    fn setup(&self) -> SetupView {
        build_setup(&clio_config::Runtime::with_env(&|_| None), None)
    }
    fn timestamp(&self) -> String {
        String::new()
    }
    fn cwd(&self) -> Option<PathBuf> {
        None
    }
    fn key_location(&self) -> Result<Option<String>, String> {
        Ok(None)
    }
}

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| (*p).to_owned()).collect()
}

#[test]
fn unknown_output_is_a_usage_error() {
    let deps = Idle;
    let error = build(&args(&["--output", "xml"]), &deps).expect_err("usage");
    assert_eq!(error.code(), "usage");
    assert!(
        error.message().contains("--output `xml`"),
        "{}",
        error.message()
    );
    assert_eq!(
        status_with(&args(&["--output", "xml"]), &deps),
        ExitCode::from(2)
    );
}

#[test]
fn bare_output_flag_is_a_usage_error() {
    let deps = Idle;
    assert_eq!(status_with(&args(&["--output"]), &deps), ExitCode::from(2));
    assert_eq!(status_with(&args(&["-o"]), &deps), ExitCode::from(2));
}

#[test]
fn short_output_alias_bad_values_are_usage_errors() {
    let deps = Idle;
    // The seam takes the arguments after the verb, and production never passes
    // the verb. A leading `status` token made the parser refuse that token
    // first, so the test passed without ever reading the `-o` value.
    for argv in [vec!["-o", "xml"], vec!["-o=xml"]] {
        let error = build(&args(&argv), &deps).expect_err("usage");
        assert_eq!(error.code(), "usage", "{argv:?}");
        assert!(
            error.message().contains("--output `xml`"),
            "{argv:?}: {}",
            error.message()
        );
    }
}

#[test]
fn help_exits_0() {
    let deps = Idle;
    for flag in ["--help", "-h", "help"] {
        assert_eq!(status_with(&args(&[flag]), &deps), ExitCode::SUCCESS);
    }
}

/// The real wiring with the one probe that touches the host's secret store
/// pinned. `clio status` really reads the key location, but a test process must
/// not: on macOS that read is a Keychain call the operating system may answer
/// with a password prompt. Every other probe delegates to `RealStatus`, so the
/// end-to-end test still exercises the real command core.
struct RealStatusNoKeystore;

impl StatusDeps for RealStatusNoKeystore {
    fn env(&self, key: &str) -> Option<String> {
        RealStatus.env(key)
    }

    fn open_store(&self, resolved: &ResolvedDatabase) -> Result<Box<dyn StatusRepos>, AmError> {
        RealStatus.open_store(resolved)
    }

    fn embedder(&self) -> Result<Option<Arc<dyn Embedder>>, AmError> {
        RealStatus.embedder()
    }

    fn scan_ports(&self) -> Vec<u16> {
        RealStatus.scan_ports()
    }

    fn compose(&self, root: &Path) -> ComposeView {
        RealStatus.compose(root)
    }

    fn setup(&self) -> SetupView {
        RealStatus.setup()
    }

    fn key_location(&self) -> Result<Option<String>, String> {
        Ok(None)
    }

    fn timestamp(&self) -> String {
        RealStatus.timestamp()
    }

    fn cwd(&self) -> Option<PathBuf> {
        RealStatus.cwd()
    }
}

#[test]
fn real_status_against_memory_store_exits_0() {
    // End-to-end over the real wiring, except the key-location probe: a test
    // must not read the host's secret store. `--backend sqlite --db
    // sqlite::memory:` removes any host database dependence; the hermetic
    // `CLIO_DEPLOYMENT_CONFIG` (set by the Makefile test/coverage targets)
    // keeps the setup view config-free.
    let code = status_with(
        &args(&[
            "--backend",
            "sqlite",
            "--db",
            "sqlite::memory:",
            "--output",
            "json",
        ]),
        &RealStatusNoKeystore,
    );
    assert_eq!(code, ExitCode::SUCCESS);
}

#[test]
fn an_undeclared_status_flag_is_a_usage_error() {
    let deps = Idle;
    let error = build(&args(&["--bogus"]), &deps).expect_err("usage");
    assert_eq!(error.code(), "usage");
    assert_eq!(status_with(&args(&["--bogus"]), &deps), ExitCode::from(2));
}

/// A global flag typed before the verb reaches this surface as a token appended
/// after its own arguments, so the surface must accept it rather than fail closed
/// on the invocation the top level just reconstructed. `--actor` is the case that
/// regressed: it is a documented global and no reserved surface names it.
#[test]
fn a_leading_global_flag_survives_the_verb_and_is_accepted() {
    let argv: Vec<String> = ["--actor", "bob", "status", "--db", "sqlite::memory:"]
        .iter()
        .map(|token| (*token).to_owned())
        .collect();
    let (rest, globals) = crate::cli_args::split_leading_globals(&argv);
    let mut it = rest.into_iter();
    assert_eq!(it.next().as_deref(), Some("status"));
    let trailing = crate::cli_args::merge_leading_globals(it, globals);
    let flags = crate::cli_top_level::parse_flags(trailing, STATUS_FLAGS).expect("accepted");
    assert_eq!(flags.get("actor").map(String::as_str), Some("bob"));
    assert_eq!(flags.get("db").map(String::as_str), Some("sqlite::memory:"));
}
